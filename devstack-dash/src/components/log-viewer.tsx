import { useEffect, useRef, useState, useMemo, useCallback } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useVirtualizer } from '@tanstack/react-virtual'
import { toast } from 'sonner'
import {
  ArrowDown,
  X,
  AlertTriangle,
  Regex,
  ListFilter,
  Share2,
  Clock,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import {
  ApiError,
  api,
  queries,
  queryKeys,
  type FacetFilter,
  type LogFilterParams,
  type RunStatusResponse,
} from '@/lib/api'
import { patchUrlParams, readUrlParam } from '@/lib/url-state'
import { buildColorIndexMap } from '@/lib/service-colors'
import {
  detectColumns,
  loadColumnConfig,
  saveColumnConfig,
  type ColumnConfig,
} from '@/lib/column-detection'
import { parseSearch, addToken, removeToken, replaceAllTokens } from '@/lib/search-parser'
import {
  LogRow,
  FacetSection,
  LogTabBar,
  LogTableHeader,
  LogScrollControls,
  LogSkeleton,
  SearchBar,
  type ParsedLog,
  type TimeRange,
} from './log-viewer/index'

interface LogViewerProps {
  runId: string
  projectDir: string
  services: string[]
  selectedService: string | null
  selectedSource?: string | null
  sourceName?: string | null
  onSelectService: (name: string | null) => void
  status?: RunStatusResponse
  isMobile?: boolean
}

// eslint-disable-next-line no-control-regex
const ANSI_RE =
  /\x1b(?:\[[0-9;?]*[A-Za-z]|\][^\x07\x1b]*(?:\x07|\x1b\\)|\([A-B]|[=>NOMDEHcZ78])/g

function stripAnsi(text: string): string {
  return text.indexOf('\x1b') === -1 ? text : text.replace(ANSI_RE, '')
}

function buildStructuredJson(entry: {
  ts: string
  service: string
  stream: string
  level: string
  message: string
  attributes?: Record<string, string>
}): Record<string, unknown> {
  const obj: Record<string, unknown> = {
    timestamp: entry.ts,
    service: entry.service,
    stream: entry.stream,
    level: entry.level || 'info',
    message: entry.message,
  }
  if (entry.attributes) {
    for (const [key, value] of Object.entries(entry.attributes)) {
      obj[key] = value
    }
  }
  return obj
}

function formatTimestamp(ts: string): string {
  if (ts.length >= 23 && ts.charCodeAt(10) === 84) return ts.slice(11, 23)
  try {
    const d = new Date(ts)
    const h = d.getHours(),
      m = d.getMinutes(),
      s = d.getSeconds(),
      ms = d.getMilliseconds()
    return `${h < 10 ? '0' : ''}${h}:${m < 10 ? '0' : ''}${m}:${s < 10 ? '0' : ''}${s}.${ms < 10 ? '00' : ms < 100 ? '0' : ''}${ms}`
  } catch {
    return ts.slice(11, 23)
  }
}

function timeRangeToSince(
  range: TimeRange,
  customSince?: string,
): string | undefined {
  if (range === 'live') return undefined
  if (range === 'custom') return customSince
  const ms = { '5m': 5 * 60_000, '15m': 15 * 60_000, '1h': 60 * 60_000 }[range]
  return new Date(Date.now() - ms).toISOString()
}

function parseSinceParam(value: string | null): {
  range: TimeRange
  customSince?: string
} {
  if (value === '5m' || value === '15m' || value === '1h')
    return { range: value }
  if (value && value.trim().length > 0)
    return { range: 'custom', customSince: value }
  return { range: 'live' }
}

function parseLastParam(value: string | null, fallback: number): number {
  if (!value) return fallback
  const parsed = Number.parseInt(value, 10)
  if (!Number.isFinite(parsed) || parsed <= 0) return fallback
  return parsed
}

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function escapeTantivyPhrase(s: string): string {
  return `"${s.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`
}

function simpleTantivyQuery(input: string, facetFields: Set<string>): string {
  const terms = input.trim().split(/\s+/).filter(Boolean)
  if (terms.length === 0) return ''
  return terms
    .map((t) => {
      const neg = t.startsWith('-')
      const raw = neg ? t.slice(1) : t
      const colon = raw.indexOf(':')
      if (colon > 0) {
        const field = raw.slice(0, colon).toLowerCase()
        const rest = raw.slice(colon + 1)
        if (facetFields.has(field) && !rest.startsWith('//'))
          return (neg ? '-' : '') + raw
        return escapeTantivyPhrase(t)
      }
      return /^[A-Za-z0-9_]+$/.test(t) ? t : escapeTantivyPhrase(t)
    })
    .join(' AND ')
}

function facetToken(field: string, value: string): string {
  if (/^[A-Za-z0-9_.-]+$/.test(value)) return `${field}:${value}`
  return `${field}:${escapeTantivyPhrase(value)}`
}

type SortDirection = 'asc' | 'desc'

const SORT_DIRECTION_STORAGE_KEY = 'devstack:log-viewer:sort-direction'
const LINE_WRAP_STORAGE_KEY = 'devstack:log-viewer:line-wrap'

function readSortDirection(): SortDirection {
  if (typeof window === 'undefined') return 'desc'
  const value = window.localStorage.getItem(SORT_DIRECTION_STORAGE_KEY)
  return value === 'asc' ? 'asc' : 'desc'
}

function readLineWrap(): boolean {
  if (typeof window === 'undefined') return false
  return window.localStorage.getItem(LINE_WRAP_STORAGE_KEY) === 'true'
}

/** Migrate legacy ?level=X and ?stream=X URL params into the search string */
function buildInitialSearch(): string {
  let search = readUrlParam('search') ?? ''
  const legacyLevel = readUrlParam('level')
  const legacyStream = readUrlParam('stream')
  if (legacyLevel && legacyLevel !== 'all') {
    search = addToken(search, 'level', legacyLevel)
  }
  if (legacyStream && legacyStream !== 'all') {
    search = addToken(search, 'stream', legacyStream)
  }
  return search
}

/* ═══════════════════════════════════════════════════════════════ */

export function LogViewer({
  runId,
  projectDir,
  services,
  selectedService,
  selectedSource,
  sourceName,
  onSelectService,
  status,
  isMobile,
}: LogViewerProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const [autoScroll, setAutoScroll] = useState(true)
  const [isAtLatest, setIsAtLatest] = useState(true)
  const [sortDirection, setSortDirection] = useState<SortDirection>(() =>
    readSortDirection(),
  )
  const [lineWrap, setLineWrap] = useState(() => readLineWrap())
  const defaultLast = 500
  const [searchInput, setSearchInput] = useState(() => buildInitialSearch())
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [isAdvancedQuery, setIsAdvancedQuery] = useState(false)
  const [facetsOpen, setFacetsOpen] = useState(false)
  const parsedSince = useMemo(() => parseSinceParam(readUrlParam('since')), [])
  const [timeRange, setTimeRange] = useState<TimeRange>(parsedSince.range)
  const [customSince] = useState<string | undefined>(parsedSince.customSince)
  const last = useMemo(
    () => parseLastParam(readUrlParam('last'), defaultLast),
    [],
  )
  const [expandedRow, setExpandedRow] = useState<number | null>(null)
  const [activeMatchIndex, setActiveMatchIndex] = useState(0)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const [newLogCount, setNewLogCount] = useState(0)
  const prevLogCountRef = useRef(0)
  const activeSourceName = sourceName ?? selectedSource ?? null
  const isSourceView = !!activeSourceName

  // ── Derive level/stream from search tokens (search string is the single source of truth) ──
  const parsedSearchTokens = useMemo(() => parseSearch(searchInput), [searchInput])
  const derivedLevel = useMemo(() => {
    const tok = parsedSearchTokens.tokens.find((t) => t.field === 'level' && !t.negated)
    return tok?.value
  }, [parsedSearchTokens])
  const derivedStream = useMemo(() => {
    const tok = parsedSearchTokens.tokens.find((t) => t.field === 'stream' && !t.negated)
    return tok?.value
  }, [parsedSearchTokens])

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(searchInput), 150)
    return () => clearTimeout(timer)
  }, [searchInput])

  useEffect(() => {
    window.localStorage.setItem(SORT_DIRECTION_STORAGE_KEY, sortDirection)
  }, [sortDirection])

  useEffect(() => {
    window.localStorage.setItem(LINE_WRAP_STORAGE_KEY, String(lineWrap))
  }, [lineWrap])

  // Clean up legacy URL params on mount
  useEffect(() => {
    const hasLegacy = readUrlParam('level') || readUrlParam('stream')
    if (hasLegacy) {
      patchUrlParams({ level: undefined, stream: undefined })
    }
  }, [])

  const selectedServiceIsValid =
    selectedService === null ||
    isSourceView ||
    selectedService.startsWith('task:') ||
    services.includes(selectedService)
  const activeTab =
    selectedService !== null && selectedServiceIsValid
      ? selectedService
      : '__all__'

  useEffect(() => {
    if (selectedService !== null && !selectedServiceIsValid)
      onSelectService(null)
  }, [selectedService, selectedServiceIsValid, onSelectService])

  useEffect(() => {
    patchUrlParams({
      search: searchInput || undefined,
      since:
        timeRange === 'custom'
          ? customSince
          : timeRange !== 'live'
            ? timeRange
            : undefined,
      last: last !== defaultLast ? last : undefined,
    })
  }, [searchInput, timeRange, customSince, last, defaultLast])

  // Facet/filter params — level and stream derived from search tokens
  const facetFilters: Omit<LogFilterParams, 'last' | 'search'> = useMemo(() => {
    const p: Omit<LogFilterParams, 'last' | 'search'> = {}
    const since = timeRangeToSince(timeRange, customSince)
    if (since) p.since = since
    if (activeTab !== '__all__') p.service = activeTab
    if (derivedLevel) p.level = derivedLevel
    if (derivedStream) p.stream = derivedStream
    return p
  }, [timeRange, customSince, activeTab, derivedLevel, derivedStream])

  const facetsQuery = useQuery({
    queryKey: isSourceView
      ? queryKeys.sourceLogsFacets(activeSourceName || '', facetFilters)
      : queryKeys.runLogsFacets(runId, facetFilters),
    queryFn: () =>
      isSourceView
        ? api.sourceLogFacets(activeSourceName || '', facetFilters)
        : api.runLogFacets(runId, facetFilters),
    enabled: isSourceView ? !!activeSourceName : !!runId,
    refetchInterval: (query) =>
      query.state.error instanceof ApiError && query.state.error.status === 404
        ? false
        : 5000,
    retry: (count, error) =>
      error instanceof ApiError && error.status === 404 ? false : count < 3,
  })

  const facetFieldSet = useMemo(() => {
    const fields = facetsQuery.data?.filters.map((filter) => filter.field) ?? []
    return new Set(fields)
  }, [facetsQuery.data])

  const serverQuery = useMemo(() => {
    if (!debouncedSearch) return undefined
    return isAdvancedQuery
      ? debouncedSearch
      : simpleTantivyQuery(debouncedSearch, facetFieldSet)
  }, [debouncedSearch, isAdvancedQuery, facetFieldSet])

  const filterParams: LogFilterParams = useMemo(() => {
    const p: LogFilterParams = { last }
    if (serverQuery) p.search = serverQuery
    if (derivedLevel) p.level = derivedLevel
    if (derivedStream) p.stream = derivedStream
    const since = timeRangeToSince(timeRange, customSince)
    if (since) p.since = since
    if (activeTab !== '__all__') p.service = activeTab
    return p
  }, [last, serverQuery, derivedLevel, timeRange, customSince, derivedStream, activeTab])

  const logsQuery = useQuery({
    queryKey: isSourceView
      ? queryKeys.sourceLogsSearch(activeSourceName || '', filterParams)
      : queryKeys.runLogsSearch(runId, filterParams),
    queryFn: () =>
      isSourceView
        ? api.searchSourceLogs(activeSourceName || '', filterParams)
        : api.searchRunLogs(runId, filterParams),
    enabled: isSourceView ? !!activeSourceName : !!runId,
    refetchInterval: (query) =>
      query.state.error instanceof ApiError && query.state.error.status === 404
        ? false
        : 1500,
    refetchOnWindowFocus: true,
    retry: (count, error) =>
      error instanceof ApiError && error.status === 404 ? false : count < 3,
  })

  const latestAgentSessionQuery = useQuery({
    ...queries.latestAgentSession(projectDir),
    enabled: Boolean(projectDir) && !isSourceView,
  })

  const shareCommand = useMemo(() => {
    if (isSourceView) return ''
    const args = ['devstack', 'show', '--run', runId]
    if (activeTab !== '__all__') args.push('--service', activeTab)
    if (searchInput.trim()) args.push('--search', searchInput.trim())
    if (timeRange === 'custom') {
      if (customSince?.trim()) args.push('--since', customSince.trim())
    } else if (timeRange !== 'live') args.push('--since', timeRange)
    if (last !== defaultLast) args.push('--last', String(last))
    return args
      .map((arg) => (arg.includes(' ') ? JSON.stringify(arg) : arg))
      .join(' ')
  }, [activeTab, customSince, defaultLast, isSourceView, last, runId, searchInput, timeRange])

  const canShare =
    !isSourceView && Boolean(latestAgentSessionQuery.data?.session)

  const shareCurrentView = useCallback(async () => {
    if (!canShare) return
    try {
      await api.shareToAgent(
        projectDir,
        shareCommand,
        'Can you take a look at this?',
      )
      toast.success('Shared log query with active agent')
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Unknown error'
      toast.error(`Failed to share query: ${message}`)
    }
  }, [canShare, projectDir, shareCommand])

  const { logs, matchCount, truncated, matchedTotal } = useMemo(() => {
    const entries = logsQuery.data?.entries ?? []
    const orderedEntries =
      sortDirection === 'desc' ? [...entries].reverse() : entries
    const result: ParsedLog[] = orderedEntries.map((e) => ({
      timestamp: formatTimestamp(e.ts),
      rawTimestamp: e.ts,
      content: stripAnsi(e.message),
      service: e.service,
      stream: e.stream,
      level: (e.level as ParsedLog['level']) || 'info',
      raw: stripAnsi(e.raw),
      json: buildStructuredJson(e),
      attributes: e.attributes,
    }))
    return {
      logs: result,
      matchCount: debouncedSearch ? result.length : 0,
      truncated: logsQuery.data?.truncated ?? false,
      matchedTotal: logsQuery.data?.matched_total ?? 0,
    }
  }, [logsQuery.data, debouncedSearch, sortDirection])

  const logServiceNames = useMemo(
    () => Array.from(new Set(logs.map((log) => log.service))),
    [logs],
  )

  // Service color mapping — deterministic via hash
  const colorIndexMap = useMemo(
    () => buildColorIndexMap(services.length > 0 ? services : logServiceNames),
    [services, logServiceNames],
  )

  // ── Column detection and management ──
  const columnStorageKey = `devstack:columns:${activeSourceName || projectDir || 'default'}`
  const rawEntries = logsQuery.data?.entries ?? []

  const [columnConfig, setColumnConfig] = useState<ColumnConfig[]>(() => {
    const saved = loadColumnConfig(columnStorageKey)
    return saved ?? []
  })

  // Re-detect columns when entries change significantly
  const columnDetectionKey = useMemo(() => {
    const fields = new Set<string>()
    for (const entry of rawEntries) {
      if (entry.attributes) {
        for (const key of Object.keys(entry.attributes)) {
          fields.add(key)
        }
      }
    }
    return Array.from(fields).sort().join(',')
  }, [rawEntries])

  useEffect(() => {
    if (rawEntries.length === 0) return
    const saved = loadColumnConfig(columnStorageKey)
    const detected = detectColumns(rawEntries, saved)
    setColumnConfig(detected)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [columnDetectionKey, columnStorageKey])

  const visibleDynamicColumns = useMemo(
    () => columnConfig.filter((c) => c.visible && !c.builtIn),
    [columnConfig],
  )

  const attributeCardinality = useMemo(() => {
    const map = new Map<string, Set<string>>()
    for (const entry of rawEntries) {
      if (!entry.attributes) continue
      for (const [key, value] of Object.entries(entry.attributes)) {
        const set = map.get(key) ?? new Set<string>()
        set.add(value)
        map.set(key, set)
      }
    }
    const result = new Map<string, number>()
    for (const [key, set] of map) {
      result.set(key, set.size)
    }
    return result
  }, [rawEntries])

  const handleToggleColumn = useCallback(
    (field: string) => {
      setColumnConfig((prev) => {
        const next = prev.map((c) =>
          c.field === field ? { ...c, visible: !c.visible } : c,
        )
        saveColumnConfig(columnStorageKey, next)
        return next
      })
    },
    [columnStorageKey],
  )

  const handleRemoveColumn = useCallback(
    (field: string) => {
      setColumnConfig((prev) => {
        const next = prev.map((c) =>
          c.field === field ? { ...c, visible: false } : c,
        )
        saveColumnConfig(columnStorageKey, next)
        return next
      })
    },
    [columnStorageKey],
  )

  const handleResizeColumn = useCallback(
    (field: string, width: number) => {
      setColumnConfig((prev) => {
        const next = prev.map((c) =>
          c.field === field ? { ...c, width } : c,
        )
        saveColumnConfig(columnStorageKey, next)
        return next
      })
    },
    [columnStorageKey],
  )

  // Track new logs when not at the latest edge.
  useEffect(() => {
    if (autoScroll || isAtLatest) {
      setNewLogCount(0)
      prevLogCountRef.current = logs.length
    } else if (logs.length > prevLogCountRef.current) {
      setNewLogCount(logs.length - prevLogCountRef.current)
    }
  }, [logs.length, autoScroll, isAtLatest])

  // Virtualizer
  const virtualizer = useVirtualizer({
    count: logs.length,
    getScrollElement: () => containerRef.current,
    estimateSize: () => 24,
    overscan: 30,
  })

  useEffect(() => {
    if (activeMatchIndex >= matchCount && matchCount > 0)
      setActiveMatchIndex(matchCount - 1)
    else if (matchCount === 0) setActiveMatchIndex(0)
  }, [matchCount, activeMatchIndex])

  useEffect(() => {
    if (matchCount === 0 || !debouncedSearch) return
    virtualizer.scrollToIndex(activeMatchIndex, { align: 'center' })
    setAutoScroll(false)
  }, [activeMatchIndex, matchCount, debouncedSearch, virtualizer])

  useEffect(() => {
    if (autoScroll && !debouncedSearch && logs.length > 0) {
      virtualizer.scrollToIndex(sortDirection === 'desc' ? 0 : logs.length - 1, {
        align: sortDirection === 'desc' ? 'start' : 'end',
      })
    }
  }, [logs, autoScroll, debouncedSearch, sortDirection, virtualizer])

  const handleScroll = useCallback(() => {
    if (!containerRef.current) return
    const { scrollTop, scrollHeight, clientHeight } = containerRef.current
    const atLatest =
      sortDirection === 'desc'
        ? scrollTop < 50
        : scrollHeight - scrollTop - clientHeight < 50
    setIsAtLatest(atLatest)
    if (atLatest && !autoScroll) setAutoScroll(true)
    else if (!atLatest && autoScroll) setAutoScroll(false)
  }, [autoScroll, sortDirection])

  const scrollToLatest = useCallback(() => {
    if (logs.length > 0) {
      virtualizer.scrollToIndex(sortDirection === 'desc' ? 0 : logs.length - 1, {
        align: sortDirection === 'desc' ? 'start' : 'end',
      })
    }
    setAutoScroll(true)
    setNewLogCount(0)
    prevLogCountRef.current = logs.length
  }, [logs.length, sortDirection, virtualizer])

  const nextMatch = useCallback(() => {
    if (matchCount === 0) return
    setActiveMatchIndex((prev) => (prev + 1) % matchCount)
  }, [matchCount])

  const prevMatch = useCallback(() => {
    if (matchCount === 0) return
    setActiveMatchIndex((prev) => (prev - 1 + matchCount) % matchCount)
  }, [matchCount])

  const toggleExpand = useCallback((index: number) => {
    setExpandedRow((prev) => (prev === index ? null : index))
  }, [])

  const resetActiveMatchIndex = useCallback(() => {
    setActiveMatchIndex(0)
  }, [])

  // ── Keyboard shortcuts ──
  // E/W now toggle level:error / level:warn tokens in the search string
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const isInput =
        document.activeElement?.tagName === 'INPUT' ||
        document.activeElement?.tagName === 'TEXTAREA'

      if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
        e.preventDefault()
        searchInputRef.current?.focus()
        searchInputRef.current?.select()
      }
      if (e.key === 'Escape') {
        if (searchInput) {
          setSearchInput('')
          setExpandedRow(null)
        }
        searchInputRef.current?.blur()
      }
      if (debouncedSearch && matchCount > 0) {
        if (e.key === 'Enter' || ((e.ctrlKey || e.metaKey) && e.key === 'g')) {
          if (document.activeElement === searchInputRef.current || !isInput) {
            e.preventDefault()
            if (e.shiftKey) prevMatch()
            else nextMatch()
          }
        }
      }
      if (e.key === '/' && !e.ctrlKey && !e.metaKey && !isInput) {
        e.preventDefault()
        searchInputRef.current?.focus()
      }
      if (!isInput && !e.ctrlKey && !e.metaKey) {
        if (e.key === 'e' || e.key === 'E') {
          e.preventDefault()
          setSearchInput((current) => {
            const parsed = parseSearch(current)
            const existing = parsed.tokens.find(
              (t) => t.field === 'level' && t.value === 'error' && !t.negated,
            )
            if (existing) return removeToken(current, existing.raw)
            return replaceAllTokens(current, 'level', 'error')
          })
        }
        if (e.key === 'w' || e.key === 'W') {
          e.preventDefault()
          setSearchInput((current) => {
            const parsed = parseSearch(current)
            const existing = parsed.tokens.find(
              (t) => t.field === 'level' && t.value === 'warn' && !t.negated,
            )
            if (existing) return removeToken(current, existing.raw)
            return replaceAllTokens(current, 'level', 'warn')
          })
        }
        if (e.key === 'f' && !e.shiftKey) {
          e.preventDefault()
          setFacetsOpen((v) => !v)
        }
        if (e.key >= '1' && e.key <= '9') {
          const idx = Number(e.key) - 1
          if (idx === 0) onSelectService(null)
          else if (idx - 1 < services.length) onSelectService(services[idx - 1])
        }
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [searchInput, debouncedSearch, matchCount, nextMatch, prevMatch, onSelectService, services])

  const showServiceColumn =
    activeTab === '__all__' &&
    (services.length > 1 || logServiceNames.length > 1)

  // Compute service column width based on longest name (~7.8px per char at 13px mono + 16px padding)
  const serviceColumnWidth = useMemo(() => {
    const names = services.length > 0 ? services : logServiceNames
    const maxLen = Math.max(...names.map((n) => n.length), 0)
    const charWidth = 7.8
    const padding = 16
    const minWidth = 96  // w-24
    const maxWidth = 200
    return Math.min(maxWidth, Math.max(minWidth, Math.ceil(maxLen * charWidth + padding)))
  }, [services, logServiceNames])

  const highlighter = useMemo(() => {
    if (!debouncedSearch) return null
    if (isAdvancedQuery) return null
    const terms = debouncedSearch.trim().split(/\s+/).filter(Boolean)
    const highlightTerms = terms.filter(
      (t) => !t.includes(':') && !t.startsWith('-'),
    )
    if (highlightTerms.length === 0) return null
    if (highlightTerms.length <= 1) return highlightTerms[0]
    const pattern = highlightTerms.map(escapeRegex).join('|')
    try {
      return new RegExp(pattern)
    } catch {
      return highlightTerms[0]
    }
  }, [debouncedSearch, isAdvancedQuery])

  const tokenParts = useMemo(
    () => searchInput.trim().split(/\s+/).filter(Boolean),
    [searchInput],
  )

  // ── Facet interaction ──
  // All facet clicks modify the search string — the single source of truth.

  const isFacetValueActive = useCallback(
    (field: string, value: string) => {
      if (field === 'service') return activeTab === value
      // For level, stream, and all other fields: check search tokens
      return tokenParts.includes(facetToken(field, value))
    },
    [activeTab, tokenParts],
  )

  const toggleFacet = useCallback(
    (field: string, value: string) => {
      if (field === 'service') {
        onSelectService(activeTab === value ? null : value)
        return
      }
      // For level/stream and all dynamic fields: toggle token in search string
      const token = facetToken(field, value)
      const parts = searchInput.trim().split(/\s+/).filter(Boolean)
      const idx = parts.indexOf(token)
      if (idx >= 0) {
        // Remove the token
        parts.splice(idx, 1)
        setSearchInput(parts.join(' '))
        setActiveMatchIndex(0)
        return
      }
      // For level and stream: replace any existing token of the same field
      if (field === 'level' || field === 'stream') {
        setSearchInput(replaceAllTokens(searchInput, field, value))
        setActiveMatchIndex(0)
        return
      }
      // Add as a new token
      setSearchInput(addToken(searchInput, field, value))
      setActiveMatchIndex(0)
    },
    [activeTab, onSelectService, searchInput],
  )

  const hasEverLoadedRef = useRef(false)
  if (logsQuery.data) hasEverLoadedRef.current = true
  const isInitialLoad =
    logsQuery.isLoading && !logsQuery.data && !hasEverLoadedRef.current

  /* ─── Time range options ─── */
  const timeRangeOptions = [
    { key: 'live' as const, label: 'Live' },
    { key: '5m' as const, label: '5m' },
    { key: '15m' as const, label: '15m' },
    { key: '1h' as const, label: '1h' },
  ]

  /* ─── Facets panel content (reused for both sidebar and overlay) ─── */
  const facetPanelContent = (
    <>
      <div className="px-3 py-2.5 border-b border-line-subtle sticky top-0 bg-surface-raised z-10">
        <div className="flex items-center justify-between">
          <span className="text-[11px] font-semibold tracking-wider uppercase text-ink-tertiary">
            Facets
          </span>
          <div className="flex items-center gap-2">
            <span className="text-[11px] text-ink-tertiary tabular-nums">
              {facetsQuery.data
                ? facetsQuery.data.total
                : facetsQuery.isLoading
                  ? '…'
                  : 0}
            </span>
            {/* Close button — only on mobile overlay */}
            {isMobile && (
              <button
                onClick={() => setFacetsOpen(false)}
                className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink transition-colors rounded-sm"
                aria-label="Close facets"
              >
                <X className="w-3.5 h-3.5" />
              </button>
            )}
          </div>
        </div>
        {facetsQuery.isError && (
          <div className="mt-1.5 text-[11px] text-status-red-text">
            Facets unavailable
          </div>
        )}
      </div>
      {(facetsQuery.data?.filters ?? []).map((filter: FacetFilter) => (
        <FacetSection
          key={filter.field}
          filter={filter}
          loading={facetsQuery.isLoading && !facetsQuery.data}
          onPick={(value: string) => toggleFacet(filter.field, value)}
          isActive={(value: string) =>
            isFacetValueActive(filter.field, value)
          }
        />
      ))}
    </>
  )

  return (
    <div className="flex-1 flex flex-col min-h-0 relative min-w-0">
      {/* ═══ Toolbar ═══ */}
      <div className="flex flex-col border-b border-line shrink-0 bg-surface-raised min-w-0 overflow-hidden">
        {/* Tab bar + right controls */}
        <LogTabBar
          services={services}
          activeTab={activeTab}
          status={status}
          onSelectService={onSelectService}
        >
          <div className="flex items-center gap-1.5 shrink-0">
            {canShare && (
              <button
                onClick={() => {
                  void shareCurrentView()
                }}
                className="h-8 px-2.5 flex items-center gap-1.5 border border-line text-ink-tertiary hover:text-ink hover:bg-surface-sunken rounded-md transition-colors"
                aria-label="Share query with agent"
                title="Share this log query with the active agent"
              >
                <Share2 className="w-3.5 h-3.5" />
                <span className="text-xs hidden md:inline">Share</span>
              </button>
            )}
            <span
              className="text-xs text-ink-tertiary tabular-nums px-1 hidden md:inline"
              aria-label={`${logs.length} lines`}
            >
              {logs.length} lines
            </span>
          </div>
        </LogTabBar>

        {/* Search + filters row */}
        <div className="flex items-center gap-1.5 px-2 md:px-3 py-1.5 border-t border-line-subtle min-w-0 w-full overflow-hidden">
          {/* Left group: time range + facets toggle */}
          <div className="flex items-center gap-1.5 shrink-0">
            {/* Time range pills */}
            <div
              className="flex items-center border border-line rounded-md overflow-hidden shrink-0"
              role="radiogroup"
              aria-label="Time range"
            >
              {timeRangeOptions.map(({ key, label }) => (
                <button
                  key={key}
                  role="radio"
                  aria-checked={timeRange === key}
                  onClick={() => setTimeRange(key)}
                  className={cn(
                    'px-2 h-8 text-xs font-medium transition-colors flex items-center gap-1 justify-center',
                    timeRange === key
                      ? key === 'live'
                        ? 'bg-accent/10 text-accent'
                        : 'bg-surface-sunken text-ink'
                      : 'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50',
                  )}
                >
                  {key === 'live' && (
                    <span
                      className={cn(
                        'w-1.5 h-1.5 rounded-full',
                        timeRange === 'live'
                          ? 'bg-status-green pulse-dot'
                          : 'bg-ink-tertiary',
                      )}
                    />
                  )}
                  {key !== 'live' && (
                    <Clock className="w-3 h-3 hidden md:block" />
                  )}
                  {label}
                </button>
              ))}
            </div>

            {/* Facets toggle */}
            <button
              onClick={() => setFacetsOpen((v) => !v)}
              className={cn(
                'h-8 px-2.5 flex items-center gap-1.5 border rounded-md transition-colors',
                facetsOpen
                  ? 'bg-surface-sunken text-ink border-line'
                  : 'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50 border-transparent',
              )}
              aria-pressed={facetsOpen}
              aria-expanded={facetsOpen}
              aria-haspopup="true"
              title={facetsOpen ? 'Hide facets (F)' : 'Show facets (F)'}
            >
              <ListFilter className="w-3.5 h-3.5" />
            </button>
          </div>

          {/* Rich search bar — fills remaining space */}
          <SearchBar
            value={searchInput}
            onChange={setSearchInput}
            onActiveMatchIndexReset={resetActiveMatchIndex}
            facetData={facetsQuery.data?.filters ?? []}
            isAdvancedQuery={isAdvancedQuery}
            onToggleAdvancedQuery={() => setIsAdvancedQuery((v) => !v)}
            matchCount={matchCount}
            activeMatchIndex={activeMatchIndex}
            truncated={truncated}
            matchedTotal={matchedTotal}
            onNextMatch={nextMatch}
            onPrevMatch={prevMatch}
            isMobile={isMobile}
            inputRef={searchInputRef}
          />

          {/* Right-side view controls */}
          <button
            onClick={() =>
              setSortDirection((current) =>
                current === 'desc' ? 'asc' : 'desc',
              )
            }
            className={cn(
              'w-8 h-8 flex items-center justify-center rounded-md transition-colors shrink-0',
              'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50',
            )}
            aria-label={
              sortDirection === 'desc' ? 'Newest first' : 'Oldest first'
            }
            aria-pressed={sortDirection === 'desc'}
            title={sortDirection === 'desc' ? 'Newest first' : 'Oldest first'}
          >
            <ArrowDown
              className={cn('w-4 h-4 transition-transform', sortDirection === 'asc' && 'rotate-180')}
            />
          </button>
          <button
            onClick={() => setLineWrap((current) => !current)}
            className={cn(
              'w-8 h-8 flex items-center justify-center rounded-md transition-colors shrink-0 text-sm leading-none',
              lineWrap
                ? 'text-accent bg-accent/10'
                : 'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50',
            )}
            aria-label={lineWrap ? 'Line wrap on' : 'Line wrap off'}
            aria-pressed={lineWrap}
            title={lineWrap ? 'Line wrap on' : 'Line wrap off'}
          >
            ↩
          </button>
          <button
            onClick={scrollToLatest}
            className={cn(
              'w-8 h-8 flex items-center justify-center rounded-md transition-colors shrink-0',
              autoScroll
                ? 'text-accent'
                : 'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50',
            )}
            aria-label={autoScroll ? 'Auto-scroll active' : 'Scroll to latest'}
          >
            <ArrowDown
              className={cn('w-4 h-4', sortDirection === 'desc' && 'rotate-180')}
            />
          </button>
        </div>
      </div>

      {/* ═══ Log content area with facets sidebar ═══ */}
      <div className="flex-1 min-h-0 flex relative">
        {/* ── Desktop: persistent sidebar alongside log content ── */}
        {facetsOpen && !isMobile && (
          <aside
            className="facets-sidebar facets-sidebar-enter"
            role="complementary"
            aria-label="Log facets"
          >
            {facetPanelContent}
          </aside>
        )}

        {/* ── Mobile: overlay (same as before) ── */}
        {facetsOpen && isMobile && (
          <>
            <div
              className="absolute inset-0 z-30"
              onClick={() => setFacetsOpen(false)}
              aria-hidden="true"
            />
            <aside
              className={cn(
                'absolute left-2 z-40 bg-surface-overlay border border-line shadow-xl rounded-lg overflow-auto',
                'facet-popover-enter',
                'inset-x-2 top-2 bottom-2 max-h-none',
              )}
              role="complementary"
              aria-label="Log facets"
            >
              {facetPanelContent}
            </aside>
          </>
        )}

        <div
          id="log-viewer"
          ref={containerRef}
          onScroll={handleScroll}
          className="flex-1 overflow-auto font-mono text-[13px] leading-snug min-w-0"
          role="log"
          aria-label="Service logs"
          aria-live="polite"
        >
          {isInitialLoad ? (
            <LogSkeleton />
          ) : logsQuery.isError &&
            logsQuery.error instanceof ApiError &&
            logsQuery.error.status === 404 ? (
            <div className="flex flex-col items-center justify-center h-full text-ink-secondary gap-3 px-8">
              <div className="w-12 h-12 bg-status-red-tint border border-line rounded-lg flex items-center justify-center mb-1">
                <AlertTriangle className="w-5 h-5 text-status-red-text" />
              </div>
              <span className="text-sm text-ink font-medium">
                {isSourceView ? 'Source not found' : 'Run stopped'}
              </span>
              <p className="text-xs text-ink-tertiary">
                {isSourceView
                  ? 'Logs are no longer available for this source.'
                  : 'Logs are no longer available for this run.'}
              </p>
            </div>
          ) : logsQuery.isError ? (
            <div className="flex flex-col items-center justify-center h-full text-ink-secondary gap-3 px-8">
              <div className="w-12 h-12 bg-status-red-tint border border-line rounded-lg flex items-center justify-center mb-1">
                <AlertTriangle className="w-5 h-5 text-status-red-text" />
              </div>
              <span className="text-sm text-ink font-medium">
                Log search failed
              </span>
              <pre className="max-w-[600px] w-full whitespace-pre-wrap break-words text-xs text-ink-tertiary bg-surface-sunken border border-line rounded-md p-3 font-mono">
                {logsQuery.error instanceof Error
                  ? logsQuery.error.message
                  : 'Unknown error'}
              </pre>
              <div className="flex items-center gap-3">
                <button
                  onClick={() => {
                    setSearchInput('')
                    setActiveMatchIndex(0)
                  }}
                  className="text-xs text-accent hover:underline px-3 py-1.5"
                >
                  Clear search
                </button>
                {!isAdvancedQuery && (
                  <button
                    onClick={() => setIsAdvancedQuery(true)}
                    className="text-xs text-ink-tertiary hover:text-accent flex items-center gap-1.5 transition-colors px-3 py-1.5"
                  >
                    <Regex className="w-3 h-3" /> Advanced query
                  </button>
                )}
              </div>
            </div>
          ) : logs.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-ink-secondary gap-3">
              {derivedLevel ? (
                <>
                  <span className="text-sm text-ink">
                    No {derivedLevel === 'error' ? 'errors' : 'warnings'}
                  </span>
                  <button
                    onClick={() => {
                      const tok = parsedSearchTokens.tokens.find(
                        (t) => t.field === 'level' && !t.negated,
                      )
                      if (tok) setSearchInput(removeToken(searchInput, tok.raw))
                    }}
                    className="text-xs text-accent hover:underline px-3 py-1.5"
                  >
                    Show all logs
                  </button>
                </>
              ) : debouncedSearch ? (
                <div className="flex flex-col items-center gap-2">
                  <span className="text-sm text-ink">
                    No matches for{' '}
                    <span className="font-mono text-ink-secondary">
                      {debouncedSearch}
                    </span>
                  </span>
                  {!isAdvancedQuery && (
                    <button
                      onClick={() => setIsAdvancedQuery(true)}
                      className="text-xs text-ink-tertiary hover:text-accent flex items-center gap-1.5 transition-colors mt-1"
                    >
                      <Regex className="w-3 h-3" /> Try advanced query
                    </button>
                  )}
                </div>
              ) : (
                <div className="flex flex-col items-center gap-2">
                  <div className="flex items-center gap-1">
                    <span
                      className="w-1 h-1 rounded-full bg-ink-tertiary animate-pulse"
                      style={{ animationDelay: '0ms' }}
                    />
                    <span
                      className="w-1 h-1 rounded-full bg-ink-tertiary animate-pulse"
                      style={{ animationDelay: '300ms' }}
                    />
                    <span
                      className="w-1 h-1 rounded-full bg-ink-tertiary animate-pulse"
                      style={{ animationDelay: '600ms' }}
                    />
                  </div>
                  <span className="text-sm text-ink-tertiary">
                    Waiting for output
                  </span>
                </div>
              )}
            </div>
          ) : (
            <>
              <LogTableHeader
                columns={columnConfig}
                showServiceColumn={showServiceColumn}
                serviceColumnWidth={serviceColumnWidth}
                lineWrap={lineWrap}
                attributeCardinality={attributeCardinality}
                onToggleColumn={handleToggleColumn}
                onRemoveColumn={handleRemoveColumn}
                onResizeColumn={handleResizeColumn}
              />
              <div
                style={{
                  height: virtualizer.getTotalSize(),
                  width: '100%',
                  position: 'relative',
                }}
              >
                {virtualizer.getVirtualItems().map((virtualRow) => {
                  const i = virtualRow.index
                  const log = logs[i]
                  const prevService = i > 0 ? logs[i - 1].service : null
                  const showLabel =
                    showServiceColumn && log.service !== prevService
                  const svcColorIndex = colorIndexMap.get(log.service) ?? 0

                  return (
                    <LogRow
                      key={virtualRow.key}
                      virtualRow={virtualRow}
                      measureElement={virtualizer.measureElement}
                      log={log}
                      index={i}
                      lineNumber={i + 1}
                      showLabel={showLabel}
                      showServiceColumn={showServiceColumn}
                      serviceColumnWidth={serviceColumnWidth}
                      svcColorIndex={svcColorIndex}
                      highlighter={highlighter}
                      isActiveMatch={!!debouncedSearch && i === activeMatchIndex}
                      isExpanded={expandedRow === i}
                      lineWrap={lineWrap}
                      onToggleExpand={toggleExpand}
                      hasBorderTop={showLabel && i > 0}
                      dynamicColumns={visibleDynamicColumns}
                    />
                  )
                })}
              </div>
            </>
          )}
        </div>
      </div>

      {/* New logs toast */}
      <LogScrollControls
        newLogCount={newLogCount}
        isAtLatest={isAtLatest}
        hasSearch={!!debouncedSearch}
        newestFirst={sortDirection === 'desc'}
        onScrollToLatest={scrollToLatest}
      />

      {/* Keyboard hints — desktop only */}
      <div className="desktop-only-hints absolute bottom-2 left-3 flex items-center gap-3 text-[11px] text-ink-tertiary pointer-events-none select-none opacity-50">
        <span className="flex items-center gap-1">
          <Kbd>/</Kbd> search
        </span>
        <span className="flex items-center gap-1">
          <Kbd>E</Kbd> errors
        </span>
        <span className="flex items-center gap-1">
          <Kbd>W</Kbd> warns
        </span>
        <span className="flex items-center gap-1">
          <Kbd>F</Kbd> facets
        </span>
      </div>
    </div>
  )
}

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="inline-flex items-center justify-center h-4 min-w-[16px] px-1 bg-surface-sunken border border-line-subtle rounded-sm text-[10px] font-mono">
      {children}
    </kbd>
  )
}
