import { useEffect, useRef, useState, useMemo, useCallback } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useVirtualizer } from '@tanstack/react-virtual'
import { toast } from 'sonner'
import {
  ArrowDown,
  Search,
  X,
  AlertTriangle,
  ChevronUp,
  ChevronDown,
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
  LogRow,
  FacetSection,
  LogTabBar,
  LogScrollControls,
  LogSkeleton,
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

type SuggestionKind = 'facet' | 'facetValue'
type SearchSuggestion = {
  id: string
  kind: SuggestionKind
  label: string
  description?: string
  insertText: string
}

function tokenAtCursor(
  text: string,
  cursor: number,
): { start: number; end: number; token: string } {
  const isWs = (c: string) => c === ' ' || c === '\n' || c === '\t'
  let start = cursor
  while (start > 0 && !isWs(text[start - 1])) start--
  let end = cursor
  while (end < text.length && !isWs(text[end])) end++
  return { start, end, token: text.slice(start, end) }
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
  const facetAnchorRef = useRef<HTMLDivElement>(null)
  const [autoScroll, setAutoScroll] = useState(true)
  const [isAtBottom, setIsAtBottom] = useState(true)
  const defaultLast = 500
  const [searchInput, setSearchInput] = useState(
    () => readUrlParam('search') ?? '',
  )
  const [debouncedSearch, setDebouncedSearch] = useState('')
  const [isAdvancedQuery, setIsAdvancedQuery] = useState(false)
  const [isSearchFocused, setIsSearchFocused] = useState(false)
  const [suggestionIndex, setSuggestionIndex] = useState(0)
  const [facetsOpen, setFacetsOpen] = useState(false)
  const [levelFilter, setLevelFilter] = useState(
    () => readUrlParam('level') ?? 'all',
  )
  const parsedSince = useMemo(() => parseSinceParam(readUrlParam('since')), [])
  const [timeRange, setTimeRange] = useState<TimeRange>(parsedSince.range)
  const [customSince] = useState<string | undefined>(parsedSince.customSince)
  const [streamFilter, setStreamFilter] = useState(
    () => readUrlParam('stream') ?? 'all',
  )
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

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedSearch(searchInput), 150)
    return () => clearTimeout(timer)
  }, [searchInput])

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
      level: levelFilter !== 'all' ? levelFilter : undefined,
      stream: streamFilter !== 'all' ? streamFilter : undefined,
      since:
        timeRange === 'custom'
          ? customSince
          : timeRange !== 'live'
            ? timeRange
            : undefined,
      last: last !== defaultLast ? last : undefined,
    })
  }, [
    searchInput,
    levelFilter,
    streamFilter,
    timeRange,
    customSince,
    last,
    defaultLast,
  ])

  // Facet/filter params
  const facetFilters: Omit<LogFilterParams, 'last' | 'search'> = useMemo(() => {
    const p: Omit<LogFilterParams, 'last' | 'search'> = {}
    const since = timeRangeToSince(timeRange, customSince)
    if (since) p.since = since
    if (activeTab !== '__all__') p.service = activeTab
    if (levelFilter !== 'all') p.level = levelFilter
    if (streamFilter !== 'all') p.stream = streamFilter
    return p
  }, [timeRange, customSince, activeTab, levelFilter, streamFilter])

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
    if (levelFilter !== 'all') p.level = levelFilter
    if (streamFilter !== 'all') p.stream = streamFilter
    const since = timeRangeToSince(timeRange, customSince)
    if (since) p.since = since
    if (activeTab !== '__all__') p.service = activeTab
    return p
  }, [
    last,
    serverQuery,
    levelFilter,
    timeRange,
    customSince,
    streamFilter,
    activeTab,
  ])

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
    if (levelFilter !== 'all') args.push('--level', levelFilter)
    if (streamFilter !== 'all') args.push('--stream', streamFilter)
    if (timeRange === 'custom') {
      if (customSince?.trim()) args.push('--since', customSince.trim())
    } else if (timeRange !== 'live') args.push('--since', timeRange)
    if (last !== defaultLast) args.push('--last', String(last))
    return args
      .map((arg) => (arg.includes(' ') ? JSON.stringify(arg) : arg))
      .join(' ')
  }, [
    activeTab,
    customSince,
    defaultLast,
    isSourceView,
    last,
    levelFilter,
    runId,
    searchInput,
    streamFilter,
    timeRange,
  ])

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
    const result: ParsedLog[] = entries.map((e) => ({
      timestamp: formatTimestamp(e.ts),
      rawTimestamp: e.ts,
      content: stripAnsi(e.message),
      service: e.service,
      stream: e.stream,
      level: (e.level as ParsedLog['level']) || 'info',
      raw: stripAnsi(e.raw),
      json: buildStructuredJson(e),
    }))
    return {
      logs: result,
      matchCount: debouncedSearch ? result.length : 0,
      truncated: logsQuery.data?.truncated ?? false,
      matchedTotal: logsQuery.data?.matched_total ?? 0,
    }
  }, [logsQuery.data, debouncedSearch])

  const logServiceNames = useMemo(
    () => Array.from(new Set(logs.map((log) => log.service))),
    [logs],
  )

  // Service color mapping — deterministic via hash
  const colorIndexMap = useMemo(
    () => buildColorIndexMap(services.length > 0 ? services : logServiceNames),
    [services, logServiceNames],
  )

  // Track new logs when not at bottom
  useEffect(() => {
    if (autoScroll || isAtBottom) {
      setNewLogCount(0)
      prevLogCountRef.current = logs.length
    } else if (logs.length > prevLogCountRef.current) {
      setNewLogCount(logs.length - prevLogCountRef.current)
    }
  }, [logs.length, autoScroll, isAtBottom])

  // Search suggestions
  const suggestions = useMemo<SearchSuggestion[]>(() => {
    if (!isSearchFocused) return []
    const el = searchInputRef.current
    const cursor = el?.selectionStart ?? searchInput.length
    const { token } = tokenAtCursor(searchInput, cursor)
    const neg = token.startsWith('-')
    const raw = neg ? token.slice(1) : token
    const lower = raw.toLowerCase()
    const out: SearchSuggestion[] = []
    const add = (s: Omit<SearchSuggestion, 'id'>) => {
      out.push({ id: `${s.kind}:${s.label}:${s.insertText}`, ...s })
    }

    const filters = facetsQuery.data?.filters ?? []
    const filterByField = new Map(
      filters.map((filter) => [filter.field, filter]),
    )

    const colon = raw.indexOf(':')
    if (colon >= 0) {
      const field = raw.slice(0, colon).toLowerCase()
      const valuePrefix = raw.slice(colon + 1)
      const filter = filterByField.get(field)
      if (filter) {
        const prefixLower = valuePrefix.toLowerCase()
        const filtered = filter.values.filter((v) =>
          v.value.toLowerCase().startsWith(prefixLower),
        )
        for (const v of filtered) {
          add({
            kind: 'facetValue',
            label: `${neg ? '-' : ''}${field}:${v.value}`,
            description: `${v.count}×`,
            insertText: `${neg ? '-' : ''}${field}:${v.value} `,
          })
        }
        return out.slice(0, 12)
      }
    }

    const facetKeys = Array.from(new Set(filters.map((filter) => filter.field)))
    if (token.length === 0) {
      for (const field of facetKeys) {
        add({
          kind: 'facet',
          label: `${field}:`,
          description: 'Filter',
          insertText: `${field}:`,
        })
      }
    } else {
      const facetMatches = facetKeys.filter((field) => field.startsWith(lower))
      for (const field of facetMatches) {
        add({
          kind: 'facet',
          label: `${neg ? '-' : ''}${field}:`,
          description: 'Filter',
          insertText: `${neg ? '-' : ''}${field}:`,
        })
      }
    }
    return out.slice(0, 8)
  }, [isSearchFocused, searchInput, facetsQuery.data])

  useEffect(() => {
    if (suggestionIndex >= suggestions.length) setSuggestionIndex(0)
  }, [suggestions, suggestionIndex])

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
      virtualizer.scrollToIndex(logs.length - 1, { align: 'end' })
    }
  }, [logs, autoScroll, debouncedSearch, virtualizer])

  const handleScroll = useCallback(() => {
    if (!containerRef.current) return
    const { scrollTop, scrollHeight, clientHeight } = containerRef.current
    const atBottom = scrollHeight - scrollTop - clientHeight < 50
    setIsAtBottom(atBottom)
    if (atBottom && !autoScroll) setAutoScroll(true)
    else if (!atBottom && autoScroll) setAutoScroll(false)
  }, [autoScroll])

  const scrollToBottom = useCallback(() => {
    if (logs.length > 0)
      virtualizer.scrollToIndex(logs.length - 1, { align: 'end' })
    setAutoScroll(true)
    setNewLogCount(0)
    prevLogCountRef.current = logs.length
  }, [logs.length, virtualizer])

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

  // Keyboard shortcuts
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
          setLevelFilter((c) => (c === 'error' ? 'all' : 'error'))
        }
        if (e.key === 'w' || e.key === 'W') {
          e.preventDefault()
          setLevelFilter((c) => (c === 'warn' ? 'all' : 'warn'))
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
  }, [
    searchInput,
    debouncedSearch,
    matchCount,
    nextMatch,
    prevMatch,
    onSelectService,
    services,
  ])

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

  const applySuggestion = useCallback(
    (s: SearchSuggestion) => {
      const el = searchInputRef.current
      const cursor = el?.selectionStart ?? searchInput.length
      const { start, end } = tokenAtCursor(searchInput, cursor)
      const next = `${searchInput.slice(0, start)}${s.insertText}${searchInput.slice(end)}`
      setSearchInput(next)
      setActiveMatchIndex(0)
      setSuggestionIndex(0)
      requestAnimationFrame(() => {
        const pos = start + s.insertText.length
        el?.focus()
        el?.setSelectionRange(pos, pos)
      })
    },
    [searchInput],
  )

  const tokenParts = useMemo(
    () => searchInput.trim().split(/\s+/).filter(Boolean),
    [searchInput],
  )

  // Extract dynamic field:value tokens from search for filter chips
  const dynamicFilterTokens = useMemo(() => {
    if (!searchInput.trim() || facetFieldSet.size === 0) return []
    const tokens: {
      field: string
      value: string
      raw: string
      negated: boolean
    }[] = []
    for (const part of tokenParts) {
      const neg = part.startsWith('-')
      const raw = neg ? part.slice(1) : part
      const colon = raw.indexOf(':')
      if (colon > 0) {
        const field = raw.slice(0, colon).toLowerCase()
        const rest = raw.slice(colon + 1)
        if (
          facetFieldSet.has(field) &&
          field !== 'service' &&
          field !== 'level' &&
          field !== 'stream'
        ) {
          const displayValue = rest.replace(/^"|"$/g, '').replace(/\\"/g, '"')
          tokens.push({ field, value: displayValue, raw: part, negated: neg })
        }
      }
    }
    return tokens
  }, [searchInput, tokenParts, facetFieldSet])

  const removeDynamicFilter = useCallback(
    (rawToken: string) => {
      const parts = searchInput.trim().split(/\s+/).filter(Boolean)
      const idx = parts.indexOf(rawToken)
      if (idx >= 0) {
        parts.splice(idx, 1)
        setSearchInput(parts.join(' '))
        setActiveMatchIndex(0)
      }
    },
    [searchInput],
  )

  const isFacetValueActive = useCallback(
    (field: string, value: string) => {
      if (field === 'service') return activeTab === value
      if (field === 'level') return levelFilter === value
      if (field === 'stream') return streamFilter === value
      return tokenParts.includes(facetToken(field, value))
    },
    [activeTab, levelFilter, streamFilter, tokenParts],
  )

  const toggleFacet = useCallback(
    (field: string, value: string) => {
      if (field === 'service') {
        onSelectService(activeTab === value ? null : value)
        return
      }
      if (field === 'level') {
        setLevelFilter((current) => (current === value ? 'all' : value))
        return
      }
      if (field === 'stream') {
        setStreamFilter((current) => (current === value ? 'all' : value))
        return
      }
      const token = facetToken(field, value)
      const parts = searchInput.trim().split(/\s+/).filter(Boolean)
      const idx = parts.indexOf(token)
      if (idx >= 0) {
        parts.splice(idx, 1)
        setSearchInput(parts.join(' '))
        setActiveMatchIndex(0)
        requestAnimationFrame(() => searchInputRef.current?.focus())
        return
      }
      applySuggestion({
        id: `facet:${token}`,
        kind: 'facetValue',
        label: token,
        insertText: `${token} `,
      })
    },
    [activeTab, applySuggestion, onSelectService, searchInput],
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
          {/* Left group: time range + facets + filter chips */}
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
            <div className="relative" ref={facetAnchorRef}>
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

            {/* Active filter chips */}
            {levelFilter !== 'all' && (
              <FilterChip
                label={`level:${levelFilter}`}
                tone={
                  levelFilter === 'error'
                    ? 'red'
                    : levelFilter === 'warn'
                      ? 'amber'
                      : 'default'
                }
                onRemove={() => setLevelFilter('all')}
              />
            )}
            {streamFilter !== 'all' && (
              <FilterChip
                label={`stream:${streamFilter}`}
                onRemove={() => setStreamFilter('all')}
              />
            )}
            {dynamicFilterTokens.map(({ field, value, raw, negated }) => (
              <FilterChip
                key={raw}
                label={`${negated ? '−' : ''}${field}:${value}`}
                onRemove={() => removeDynamicFilter(raw)}
              />
            ))}
          </div>

          {/* Search input — fills remaining space */}
          <div
            className={cn(
              'relative flex items-center gap-2 flex-1 min-w-0 bg-surface-base border px-2.5 h-9 rounded-md transition-colors',
              'border-line focus-within:border-accent/40 focus-within:bg-surface-raised',
            )}
          >
            <Search className="w-3.5 h-3.5 text-ink-tertiary shrink-0" />
            <input
              ref={searchInputRef}
              type="text"
              value={searchInput}
              onChange={(e) => {
                setSearchInput(e.target.value)
                setActiveMatchIndex(0)
              }}
              onFocus={() => setIsSearchFocused(true)}
              onBlur={() => {
                setTimeout(() => setIsSearchFocused(false), 100)
              }}
              onKeyDown={(e) => {
                if (!isSearchFocused || suggestions.length === 0) return
                if (e.key === 'ArrowDown') {
                  e.preventDefault()
                  e.stopPropagation()
                  setSuggestionIndex((i) =>
                    Math.min(i + 1, suggestions.length - 1),
                  )
                } else if (e.key === 'ArrowUp') {
                  e.preventDefault()
                  e.stopPropagation()
                  setSuggestionIndex((i) => Math.max(i - 1, 0))
                } else if (e.key === 'Enter' || e.key === 'Tab') {
                  e.preventDefault()
                  e.stopPropagation()
                  const s = suggestions[suggestionIndex]
                  if (s) applySuggestion(s)
                } else if (e.key === 'Escape') {
                  e.stopPropagation()
                  setIsSearchFocused(false)
                }
              }}
              placeholder={
                isMobile ? 'Search logs…' : 'Search logs…  / to focus'
              }
              className="bg-transparent text-[13px] text-ink placeholder:text-ink-tertiary outline-none flex-1 min-w-0"
              aria-label="Search log lines"
              spellCheck={false}
            />
            {/* Regex toggle inside search */}
            <button
              onClick={() => setIsAdvancedQuery(!isAdvancedQuery)}
              className={cn(
                'w-6 h-6 flex items-center justify-center rounded-sm transition-colors shrink-0',
                isAdvancedQuery
                  ? 'bg-accent/15 text-accent'
                  : 'text-ink-tertiary hover:text-ink-secondary',
              )}
              aria-pressed={isAdvancedQuery}
              title="Toggle advanced query"
            >
              <Regex className="w-3 h-3" />
            </button>
            {searchInput && (
              <>
                <div className="flex items-center gap-0.5 shrink-0 border-l border-line-subtle pl-1.5 ml-0.5">
                  <span className="text-[11px] text-ink-tertiary tabular-nums mr-0.5">
                    {matchCount > 0
                      ? `${activeMatchIndex + 1}/${matchCount}`
                      : '0'}
                    {truncated && matchedTotal > matchCount
                      ? ` of ${matchedTotal}`
                      : ''}
                  </span>
                  <button
                    onClick={prevMatch}
                    disabled={matchCount === 0}
                    className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink disabled:opacity-20 transition-colors"
                    aria-label="Previous match"
                  >
                    <ChevronUp className="w-3 h-3" />
                  </button>
                  <button
                    onClick={nextMatch}
                    disabled={matchCount === 0}
                    className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink disabled:opacity-20 transition-colors"
                    aria-label="Next match"
                  >
                    <ChevronDown className="w-3 h-3" />
                  </button>
                </div>
                <button
                  onClick={() => {
                    setSearchInput('')
                    setActiveMatchIndex(0)
                  }}
                  className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink transition-colors shrink-0"
                  aria-label="Clear search"
                >
                  <X className="w-3 h-3" />
                </button>
              </>
            )}

            {/* Suggestions dropdown */}
            {isSearchFocused && suggestions.length > 0 && (
              <div
                className="absolute left-0 right-0 top-full mt-1 bg-surface-overlay border border-line shadow-xl rounded-md z-50 max-h-56 overflow-auto"
                onMouseDown={(e) => e.preventDefault()}
                role="listbox"
                aria-label="Search suggestions"
              >
                {suggestions.map((s, i) => (
                  <button
                    key={s.id}
                    type="button"
                    onClick={() => applySuggestion(s)}
                    className={cn(
                      'w-full text-left px-3 py-1.5 flex items-center justify-between gap-4',
                      'text-xs font-mono transition-colors',
                      i === suggestionIndex
                        ? 'bg-surface-sunken text-ink'
                        : 'hover:bg-surface-sunken/50 text-ink-secondary',
                    )}
                    role="option"
                    aria-selected={i === suggestionIndex}
                  >
                    <span className="truncate">{s.label}</span>
                    {s.description && (
                      <span className="text-[11px] text-ink-tertiary truncate">
                        {s.description}
                      </span>
                    )}
                  </button>
                ))}
              </div>
            )}
          </div>

          {/* Right: auto-scroll only */}
          <button
            onClick={scrollToBottom}
            className={cn(
              'w-8 h-8 flex items-center justify-center rounded-md transition-colors shrink-0',
              autoScroll
                ? 'text-accent'
                : 'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50',
            )}
            aria-label={autoScroll ? 'Auto-scroll active' : 'Scroll to latest'}
          >
            <ArrowDown className="w-4 h-4" />
          </button>
        </div>
      </div>

      {/* ═══ Log content ═══ */}
      <div className="flex-1 min-h-0 flex relative">
        {/* Facets popover — overlays log content, doesn't consume layout space */}
        {facetsOpen && (
          <>
            {/* Click-outside detection — positioned within the log area only */}
            <div
              className="absolute inset-0 z-30"
              onClick={() => setFacetsOpen(false)}
              aria-hidden="true"
            />
            <aside
              className={cn(
                'absolute left-2 z-40 bg-surface-overlay border border-line shadow-xl rounded-lg overflow-auto',
                'facet-popover-enter',
                isMobile
                  ? 'inset-x-2 top-2 bottom-2 max-h-none'
                  : 'top-2 w-72 max-h-[min(520px,calc(100%-16px))]',
              )}
              role="complementary"
              aria-label="Log facets"
            >
              <div className="px-3 py-2.5 border-b border-line-subtle sticky top-0 bg-surface-overlay z-10">
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
                    <button
                      onClick={() => setFacetsOpen(false)}
                      className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink transition-colors rounded-sm"
                      aria-label="Close facets"
                    >
                      <X className="w-3.5 h-3.5" />
                    </button>
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
              {levelFilter !== 'all' ? (
                <>
                  <span className="text-sm text-ink">
                    No {levelFilter === 'error' ? 'errors' : 'warnings'}
                  </span>
                  <button
                    onClick={() => setLevelFilter('all')}
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
                    onToggleExpand={toggleExpand}
                    hasBorderTop={showLabel && i > 0}
                  />
                )
              })}
            </div>
          )}
        </div>
      </div>

      {/* New logs toast */}
      <LogScrollControls
        newLogCount={newLogCount}
        isAtBottom={isAtBottom}
        hasSearch={!!debouncedSearch}
        onScrollToBottom={scrollToBottom}
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

function FilterChip({
  label,
  tone = 'default',
  onRemove,
}: {
  label: string
  tone?: 'default' | 'red' | 'amber'
  onRemove: () => void
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 h-6 px-2 text-[11px] font-medium rounded-full border transition-colors',
        tone === 'red' &&
          'bg-status-red-tint border-status-red/20 text-status-red-text',
        tone === 'amber' &&
          'bg-status-amber-tint border-status-amber/20 text-status-amber-text',
        tone === 'default' &&
          'bg-surface-sunken border-line text-ink-secondary',
      )}
    >
      <span className="font-mono">{label}</span>
      <button
        onClick={onRemove}
        className="w-3.5 h-3.5 flex items-center justify-center rounded-full hover:bg-black/10 transition-colors"
        aria-label={`Remove ${label} filter`}
      >
        <X className="w-2.5 h-2.5" />
      </button>
    </span>
  )
}
