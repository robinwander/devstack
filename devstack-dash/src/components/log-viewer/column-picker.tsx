import { useState, useRef, useEffect, useMemo, useCallback } from 'react'
import { Plus, Search, X, Check } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { ColumnConfig } from '@/lib/column-detection'

interface ColumnPickerProps {
  columns: ColumnConfig[]
  attributeCardinality: Map<string, number>
  onToggleColumn: (field: string) => void
}

export function ColumnPicker({
  columns,
  attributeCardinality,
  onToggleColumn,
}: ColumnPickerProps) {
  const [open, setOpen] = useState(false)
  const [search, setSearch] = useState('')
  const anchorRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  // Close on outside click
  useEffect(() => {
    if (!open) return
    const handler = (e: MouseEvent) => {
      if (
        panelRef.current &&
        !panelRef.current.contains(e.target as Node) &&
        anchorRef.current &&
        !anchorRef.current.contains(e.target as Node)
      ) {
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [open])

  // Focus search on open
  useEffect(() => {
    if (open) {
      requestAnimationFrame(() => searchRef.current?.focus())
    } else {
      setSearch('')
    }
  }, [open])

  // Close on Escape
  useEffect(() => {
    if (!open) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [open])

  const dynamicColumns = useMemo(
    () => columns.filter((c) => !c.builtIn),
    [columns],
  )

  const searchLower = search.toLowerCase()

  const { suggested, other } = useMemo(() => {
    const filtered = dynamicColumns.filter((c) =>
      c.field.toLowerCase().includes(searchLower) ||
      c.label.toLowerCase().includes(searchLower),
    )

    // Suggested: visible or top-ranked (first 5 by detection order)
    const suggestedSet = new Set(
      filtered.filter((c) => c.visible).map((c) => c.field),
    )
    // Add top non-visible ones if we have few suggested
    for (const c of filtered) {
      if (suggestedSet.size >= 5) break
      suggestedSet.add(c.field)
    }

    const suggested = filtered.filter((c) => suggestedSet.has(c.field))
    const other = filtered.filter((c) => !suggestedSet.has(c.field))

    return { suggested, other }
  }, [dynamicColumns, searchLower])

  const handleToggle = useCallback(
    (field: string) => {
      onToggleColumn(field)
    },
    [onToggleColumn],
  )

  if (dynamicColumns.length === 0) return null

  return (
    <div className="relative">
      <button
        ref={anchorRef}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          'h-full px-1.5 flex items-center justify-center transition-colors',
          'text-ink-tertiary hover:text-ink hover:bg-surface-sunken/50',
          open && 'text-ink bg-surface-sunken/50',
        )}
        aria-label="Add column"
        aria-expanded={open}
        title="Add or remove columns"
      >
        <Plus className="w-3.5 h-3.5" />
      </button>

      {open && (
        <div
          ref={panelRef}
          className="absolute right-0 top-full mt-1 w-64 bg-surface-overlay border border-line shadow-xl rounded-md z-50 overflow-hidden column-picker-enter"
        >
          {/* Search */}
          <div className="flex items-center gap-2 px-2.5 py-2 border-b border-line-subtle">
            <Search className="w-3.5 h-3.5 text-ink-tertiary shrink-0" />
            <input
              ref={searchRef}
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Filter fields…"
              className="bg-transparent text-xs text-ink placeholder:text-ink-tertiary outline-none flex-1 min-w-0"
              spellCheck={false}
            />
            {search && (
              <button
                onClick={() => setSearch('')}
                className="w-4 h-4 flex items-center justify-center text-ink-tertiary hover:text-ink"
              >
                <X className="w-3 h-3" />
              </button>
            )}
          </div>

          {/* Column list */}
          <div className="max-h-64 overflow-auto py-1">
            {suggested.length === 0 && other.length === 0 ? (
              <div className="px-3 py-3 text-xs text-ink-tertiary text-center">
                No fields match
              </div>
            ) : (
              <>
                {suggested.length > 0 && (
                  <ColumnGroup
                    label="Suggested"
                    columns={suggested}
                    attributeCardinality={attributeCardinality}
                    onToggle={handleToggle}
                  />
                )}
                {other.length > 0 && (
                  <ColumnGroup
                    label="All fields"
                    columns={other}
                    attributeCardinality={attributeCardinality}
                    onToggle={handleToggle}
                  />
                )}
              </>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

function ColumnGroup({
  label,
  columns,
  attributeCardinality,
  onToggle,
}: {
  label: string
  columns: ColumnConfig[]
  attributeCardinality: Map<string, number>
  onToggle: (field: string) => void
}) {
  return (
    <div>
      <div className="px-3 pt-2 pb-1">
        <span className="text-[10px] font-semibold tracking-wider uppercase text-ink-tertiary">
          {label}
        </span>
      </div>
      {columns.map((col) => {
        const cardinality = attributeCardinality.get(col.field)
        return (
          <button
            key={col.field}
            onClick={() => onToggle(col.field)}
            className={cn(
              'w-full flex items-center gap-2 px-3 py-1.5 text-xs transition-colors',
              'hover:bg-surface-sunken/50',
              col.visible && 'bg-surface-sunken/30',
            )}
          >
            <span
              className={cn(
                'w-4 h-4 flex items-center justify-center rounded-sm border shrink-0 transition-colors',
                col.visible
                  ? 'bg-accent border-accent text-white'
                  : 'border-line',
              )}
            >
              {col.visible && <Check className="w-3 h-3" />}
            </span>
            <span className="font-mono text-ink-secondary truncate flex-1 text-left">
              {col.field}
            </span>
            {cardinality !== undefined && (
              <span className="text-[11px] text-ink-tertiary tabular-nums shrink-0">
                {cardinality}
              </span>
            )}
          </button>
        )
      })}
    </div>
  )
}
