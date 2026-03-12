import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Search, X, Regex, ChevronUp, ChevronDown } from 'lucide-react'
import { cn } from '@/lib/utils'
import { parseSearch, type SearchToken } from '@/lib/search-parser'
import type { FacetFilter } from '@/lib/api'

/* ═══════════════════════════════════════════════════════════════════
   SearchBar — rich input with inline token highlighting & intellisense

   The input value is always the raw search string.
   An overlay renders field:value tokens with colored pill backgrounds,
   aligned character-for-character with the monospace input text.
   ═══════════════════════════════════════════════════════════════════ */

export interface SearchBarProps {
  value: string
  onChange: (value: string) => void
  onActiveMatchIndexReset: () => void
  facetData: FacetFilter[]
  isAdvancedQuery: boolean
  onToggleAdvancedQuery: () => void
  matchCount: number
  activeMatchIndex: number
  truncated: boolean
  matchedTotal: number
  onNextMatch: () => void
  onPrevMatch: () => void
  isMobile?: boolean
  inputRef: React.RefObject<HTMLInputElement | null>
}

// ── Overlay segment types ──

interface TokenSegment {
  type: 'token'
  text: string
  token: SearchToken
}
interface TextSegment {
  type: 'text'
  text: string
}
type Segment = TokenSegment | TextSegment

function buildSegments(input: string, tokens: SearchToken[]): Segment[] {
  if (!input || tokens.length === 0) return [{ type: 'text', text: input }]

  const segments: Segment[] = []
  let pos = 0
  const sorted = [...tokens].sort((a, b) => a.start - b.start)

  for (const token of sorted) {
    if (token.start > pos) {
      segments.push({ type: 'text', text: input.slice(pos, token.start) })
    }
    segments.push({ type: 'token', text: input.slice(token.start, token.end), token })
    pos = token.end
  }
  if (pos < input.length) {
    segments.push({ type: 'text', text: input.slice(pos) })
  }
  return segments
}

// ── Token color classification ──

function tokenColorClass(field: string, value: string, negated: boolean): string {
  if (negated) return 'search-token-muted'
  if (field === 'level') {
    if (value === 'error') return 'search-token-red'
    if (value === 'warn' || value === 'warning') return 'search-token-amber'
  }
  if (field === 'stream' && value === 'stderr') return 'search-token-red'
  return 'search-token-accent'
}

// ── Cursor-aware word extraction ──

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

// ── Suggestion types ──

interface SearchSuggestion {
  id: string
  kind: 'facet' | 'facetValue'
  label: string
  detail?: string
  insertText: string
}

// ═══════════════════════════════════════════════════════════════════

export function SearchBar({
  value,
  onChange,
  onActiveMatchIndexReset,
  facetData,
  isAdvancedQuery,
  onToggleAdvancedQuery,
  matchCount,
  activeMatchIndex,
  truncated,
  matchedTotal,
  onNextMatch,
  onPrevMatch,
  isMobile,
  inputRef,
}: SearchBarProps) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const [isFocused, setIsFocused] = useState(false)
  const [suggestionIndex, setSuggestionIndex] = useState(0)

  // Parse tokens from value (skip in advanced mode)
  const parsed = useMemo(
    () => (isAdvancedQuery ? { tokens: [] as SearchToken[], freeText: value } : parseSearch(value)),
    [value, isAdvancedQuery],
  )
  const hasTokens = parsed.tokens.length > 0

  // Build overlay segments
  const segments = useMemo(
    () => (hasTokens && !isAdvancedQuery ? buildSegments(value, parsed.tokens) : []),
    [value, parsed.tokens, hasTokens, isAdvancedQuery],
  )

  // Sync overlay scroll with input scroll
  const syncScroll = useCallback(() => {
    if (overlayRef.current && inputRef.current) {
      overlayRef.current.scrollLeft = inputRef.current.scrollLeft
    }
  }, [inputRef])

  // ── Suggestions ──

  const suggestions = useMemo<SearchSuggestion[]>(() => {
    if (!isFocused || isAdvancedQuery) return []

    const el = inputRef.current
    const cursor = el?.selectionStart ?? value.length
    const { token } = tokenAtCursor(value, cursor)

    const neg = token.startsWith('-')
    const raw = neg ? token.slice(1) : token
    const lower = raw.toLowerCase()

    const out: SearchSuggestion[] = []
    const filterByField = new Map(facetData.map((f) => [f.field, f]))
    const facetKeys = facetData.map((f) => f.field)

    const colon = raw.indexOf(':')
    if (colon >= 0) {
      // Typing a value — suggest matching values
      const field = raw.slice(0, colon).toLowerCase()
      const valuePrefix = raw.slice(colon + 1).toLowerCase()
      const filter = filterByField.get(field)
      if (filter) {
        for (const v of filter.values) {
          if (v.value.toLowerCase().startsWith(valuePrefix)) {
            out.push({
              id: `val:${neg ? '-' : ''}${field}:${v.value}`,
              kind: 'facetValue',
              label: `${neg ? '-' : ''}${field}:${v.value}`,
              detail: `${v.count}`,
              insertText: `${neg ? '-' : ''}${field}:${v.value} `,
            })
          }
        }
        return out.slice(0, 12)
      }
    }

    // Suggest field names
    if (token.length === 0) {
      for (const field of facetKeys) {
        out.push({
          id: `field:${field}`,
          kind: 'facet',
          label: `${field}:`,
          detail: 'field',
          insertText: `${field}:`,
        })
      }
    } else {
      for (const field of facetKeys) {
        if (field.startsWith(lower)) {
          out.push({
            id: `field:${neg ? '-' : ''}${field}`,
            kind: 'facet',
            label: `${neg ? '-' : ''}${field}:`,
            detail: 'field',
            insertText: `${neg ? '-' : ''}${field}:`,
          })
        }
      }
    }
    return out.slice(0, 8)
  }, [isFocused, isAdvancedQuery, value, inputRef, facetData])

  useEffect(() => {
    if (suggestionIndex >= suggestions.length) setSuggestionIndex(0)
  }, [suggestions, suggestionIndex])

  const applySuggestion = useCallback(
    (s: SearchSuggestion) => {
      const el = inputRef.current
      const cursor = el?.selectionStart ?? value.length
      const { start, end } = tokenAtCursor(value, cursor)
      const next = `${value.slice(0, start)}${s.insertText}${value.slice(end)}`
      onChange(next)
      onActiveMatchIndexReset()
      setSuggestionIndex(0)
      requestAnimationFrame(() => {
        const pos = start + s.insertText.length
        el?.focus()
        el?.setSelectionRange(pos, pos)
      })
    },
    [value, onChange, onActiveMatchIndexReset, inputRef],
  )

  // ── Keyboard ──

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (isFocused && suggestions.length > 0) {
        if (e.key === 'ArrowDown') {
          e.preventDefault()
          e.stopPropagation()
          setSuggestionIndex((i) => Math.min(i + 1, suggestions.length - 1))
          return
        }
        if (e.key === 'ArrowUp') {
          e.preventDefault()
          e.stopPropagation()
          setSuggestionIndex((i) => Math.max(i - 1, 0))
          return
        }
        if (e.key === 'Tab' || e.key === 'Enter') {
          const s = suggestions[suggestionIndex]
          if (s) {
            e.preventDefault()
            e.stopPropagation()
            applySuggestion(s)
            return
          }
        }
      }
      if (e.key === 'Escape') {
        e.stopPropagation()
        setIsFocused(false)
      }
    },
    [isFocused, suggestions, suggestionIndex, applySuggestion],
  )

  // ── Handlers ──

  const handleChange = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      onChange(e.target.value)
      onActiveMatchIndexReset()
      requestAnimationFrame(syncScroll)
    },
    [onChange, onActiveMatchIndexReset, syncScroll],
  )

  const handleClear = useCallback(() => {
    onChange('')
    onActiveMatchIndexReset()
  }, [onChange, onActiveMatchIndexReset])

  const handleFocus = useCallback(() => setIsFocused(true), [])
  const handleBlur = useCallback(() => {
    setTimeout(() => setIsFocused(false), 120)
  }, [])

  const handleContainerClick = useCallback(() => {
    inputRef.current?.focus()
  }, [inputRef])

  const hasValue = value.length > 0

  return (
    <div
      onClick={handleContainerClick}
      className={cn(
        'search-bar relative flex items-center gap-2 flex-1 min-w-0 bg-surface-base border px-2.5 h-9 rounded-md transition-colors cursor-text',
        'border-line focus-within:border-accent/40 focus-within:bg-surface-raised',
      )}
    >
      <Search className="w-3.5 h-3.5 text-ink-tertiary shrink-0" />

      {/* ── Input area with token overlay ── */}
      <div className="relative flex-1 min-w-0 h-full flex items-center">
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={handleChange}
          onFocus={handleFocus}
          onBlur={handleBlur}
          onKeyDown={handleKeyDown}
          onScroll={syncScroll}
          placeholder={isMobile ? 'Search logs…' : 'Search logs…  / to focus'}
          className={cn(
            'w-full bg-transparent text-[13px] font-mono outline-none min-w-0 p-0',
            hasTokens && !isAdvancedQuery
              ? 'text-transparent placeholder:text-ink-tertiary selection:bg-accent/20'
              : 'text-ink placeholder:text-ink-tertiary',
          )}
          style={
            hasTokens && !isAdvancedQuery
              ? { caretColor: 'var(--text-primary)' }
              : undefined
          }
          aria-label="Search log lines"
          spellCheck={false}
        />

        {/* Syntax-highlighted overlay — aligned with input text */}
        {hasTokens && !isAdvancedQuery && (
          <div
            ref={overlayRef}
            className="search-overlay absolute inset-0 flex items-center font-mono text-[13px] pointer-events-none overflow-hidden whitespace-nowrap p-0"
            aria-hidden="true"
          >
            {segments.map((seg, i) =>
              seg.type === 'token' ? (
                <span
                  key={i}
                  className={cn(
                    'search-token',
                    tokenColorClass(seg.token.field, seg.token.value, seg.token.negated),
                  )}
                >
                  {seg.text}
                </span>
              ) : (
                <span key={i} className="text-ink">
                  {seg.text}
                </span>
              ),
            )}
          </div>
        )}
      </div>

      {/* Regex toggle */}
      <button
        type="button"
        onClick={onToggleAdvancedQuery}
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

      {/* Match navigation — only when there's a value */}
      {hasValue && (
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
              type="button"
              onClick={onPrevMatch}
              disabled={matchCount === 0}
              className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink disabled:opacity-20 transition-colors"
              aria-label="Previous match"
            >
              <ChevronUp className="w-3 h-3" />
            </button>
            <button
              type="button"
              onClick={onNextMatch}
              disabled={matchCount === 0}
              className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink disabled:opacity-20 transition-colors"
              aria-label="Next match"
            >
              <ChevronDown className="w-3 h-3" />
            </button>
          </div>
          <button
            type="button"
            onClick={handleClear}
            className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink transition-colors shrink-0"
            aria-label="Clear search"
          >
            <X className="w-3 h-3" />
          </button>
        </>
      )}

      {/* ── Intellisense dropdown ── */}
      {isFocused && suggestions.length > 0 && (
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
              <span className="truncate">
                {s.kind === 'facetValue' ? (
                  <SearchSuggestionLabel text={s.label} />
                ) : (
                  s.label
                )}
              </span>
              {s.detail && (
                <span className="text-[11px] text-ink-tertiary tabular-nums shrink-0">
                  {s.detail}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

/** Renders a suggestion label with field dimmed and value highlighted */
function SearchSuggestionLabel({ text }: { text: string }) {
  const colon = text.indexOf(':')
  if (colon < 0) return <>{text}</>
  const field = text.slice(0, colon + 1)
  const val = text.slice(colon + 1)
  return (
    <>
      <span className="text-ink-tertiary">{field}</span>
      <span className="text-ink">{val}</span>
    </>
  )
}
