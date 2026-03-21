import { memo, useCallback } from 'react'
import { cn } from '@/lib/utils'
import { highlightAll } from './highlight'
import { LogDetail, type DetailFilterAction } from './log-detail'
import type { ParsedLog } from './types'
import type { ColumnConfig } from '@/lib/column-detection'

interface LogRowProps {
  log: ParsedLog
  index: number
  lineNumber: number
  virtualRow: { index: number; start: number; key?: string | number | bigint }
  measureElement: (el: Element | null) => void
  showLabel: boolean
  showServiceColumn: boolean
  serviceColumnWidth?: number
  svcColorIndex: number
  highlighter: string | RegExp | null
  isActiveMatch: boolean
  isExpanded: boolean
  isSelected: boolean
  isNew?: boolean
  lineWrap: boolean
  canShare: boolean
  isMobile?: boolean
  onToggleExpand: (index: number) => void
  onSelectRow: (index: number, extendRange: boolean) => void
  onShareLog: (log: ParsedLog) => void
  onFilterAction: (
    field: string,
    value: string,
    action: DetailFilterAction,
  ) => void
  hasBorderTop: boolean
  dynamicColumns: ColumnConfig[]
}

export const LogRow = memo(function LogRow({
  log,
  index,
  lineNumber,
  virtualRow,
  measureElement,
  showLabel,
  showServiceColumn,
  serviceColumnWidth,
  svcColorIndex,
  highlighter,
  isActiveMatch,
  isExpanded,
  isSelected,
  isNew,
  lineWrap,
  canShare,
  isMobile,
  onToggleExpand,
  onSelectRow,
  onShareLog,
  onFilterAction,
  hasBorderTop,
  dynamicColumns,
}: LogRowProps) {
  const level = log.level
  const svcColorClass = `svc-color-${svcColorIndex}`

  const handleClick = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      if (event.shiftKey) {
        event.preventDefault()
        onSelectRow(index, true)
        return
      }
      onToggleExpand(index)
    },
    [index, onSelectRow, onToggleExpand],
  )

  const handleLineNumberClick = useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      event.preventDefault()
      event.stopPropagation()
      onSelectRow(index, event.shiftKey)
    },
    [index, onSelectRow],
  )

  const levelTint =
    level === 'error'
      ? 'log-level-error-tint'
      : level === 'warn'
        ? 'log-level-warn-tint'
        : ''

  const levelStrip =
    level === 'error'
      ? 'log-level-error-strip'
      : level === 'warn'
        ? 'log-level-warn-strip'
        : ''

  return (
    <div
      data-index={virtualRow.index}
      ref={measureElement}
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        width: '100%',
        transform: `translateY(${virtualRow.start}px)`,
      }}
    >
      <div
        onClick={handleClick}
        data-new={isNew || undefined}
        className={cn(
          'log-line cursor-pointer flex',
          svcColorClass,
          isNew && 'log-line-new',
          hasBorderTop && 'border-t border-line-subtle',
          isActiveMatch && '!bg-accent/8',
          isExpanded && '!bg-surface-sunken',
          isSelected && '!bg-accent/10 ring-1 ring-inset ring-accent/20',
          showServiceColumn && !levelTint && 'svc-row-tint',
          levelTint,
        )}
        aria-selected={isSelected}
      >
        <button
          type="button"
          onClick={handleLineNumberClick}
          className={cn(
            'log-line-number py-[2px] pr-2 pl-1 shrink-0 hover:text-ink transition-colors text-right',
            isSelected && 'text-accent',
          )}
          style={isMobile ? { width: 32 } : undefined}
          aria-label={
            isSelected
              ? `Unselect row ${lineNumber}`
              : `Select row ${lineNumber}`
          }
          aria-pressed={isSelected}
        >
          {lineNumber}
        </button>

        <span
          className="log-ts pr-2 py-[2px] text-ink-tertiary select-none whitespace-nowrap tabular-nums text-[13px] font-mono shrink-0"
          style={{ width: isMobile ? 80 : 108 }}
        >
          {isMobile ? log.timestamp.slice(0, 8) : log.timestamp}
        </span>

        {showServiceColumn && (
          <span
            className={cn('w-[3px] min-h-full shrink-0', levelStrip || 'svc-strip-bg')}
          />
        )}

        {showServiceColumn && (
          <span
            className={cn(
              'py-[2px] px-2 select-none whitespace-nowrap shrink-0 overflow-hidden text-ellipsis text-[13px]',
              showLabel
                ? 'svc-text font-semibold'
                : 'text-ink-tertiary font-normal',
            )}
            style={serviceColumnWidth ? { width: serviceColumnWidth } : { width: 96 }}
          >
            {log.service}
          </span>
        )}

        <span className="py-[2px] px-1 shrink-0 flex items-center" style={{ width: 56 }}>
          <LevelBadge level={level} />
        </span>

        {!isMobile &&
          dynamicColumns.map((col) => {
            const value = log.attributes?.[col.field]
            return (
              <span
                key={col.field}
                className="log-attr-cell py-[2px] px-2 shrink-0 font-mono text-[13px] whitespace-nowrap overflow-hidden text-ellipsis"
                style={{ width: col.width }}
                title={value || undefined}
              >
                {value ? (
                  <span className="text-ink-secondary">{value}</span>
                ) : (
                  <span className="text-ink-tertiary/40">—</span>
                )}
              </span>
            )
          })}

        <span
          className={cn(
            'py-[2px] pl-2 pr-4 log-content-text min-w-0 flex-1 font-mono text-[13px]',
            lineWrap
              ? 'whitespace-pre-wrap break-words'
              : 'whitespace-nowrap overflow-hidden text-ellipsis',
            level === 'error' && 'text-status-red-text',
            level === 'warn' && 'text-status-amber-text',
            level === 'info' && 'text-ink-secondary',
          )}
        >
          {highlighter ? highlightAll(log.content, highlighter) : log.content}
        </span>
      </div>

      {isExpanded && (
        <LogDetail
          log={log}
          svcColorClass={svcColorClass}
          canShare={canShare}
          onShare={onShareLog}
          onFilterAction={onFilterAction}
        />
      )}
    </div>
  )
})
LogRow.displayName = 'LogRow'

function LevelBadge({ level }: { level: 'info' | 'warn' | 'error' }) {
  return (
    <span
      className={cn(
        'inline-flex items-center justify-center h-[18px] min-w-[18px] px-1 rounded text-[10px] font-bold uppercase tracking-wide leading-none',
        level === 'error' && 'bg-status-red/15 text-status-red-text',
        level === 'warn' && 'bg-status-amber/15 text-status-amber-text',
        level === 'info' && 'bg-surface-sunken text-ink-tertiary',
      )}
    >
      {level === 'error' ? 'ERR' : level === 'warn' ? 'WRN' : 'INF'}
    </span>
  )
}
