import { memo, useCallback } from 'react'
import { cn } from '@/lib/utils'
import { highlightAll } from './highlight'
import { LogDetail } from './log-detail'
import type { ParsedLog } from './types'

interface LogRowProps {
  log: ParsedLog
  index: number
  lineNumber: number
  virtualRow: { index: number; start: number }
  measureElement: (el: Element | null) => void
  showLabel: boolean
  showServiceColumn: boolean
  serviceColumnWidth?: number
  svcColorIndex: number
  highlighter: string | RegExp | null
  isActiveMatch: boolean
  isExpanded: boolean
  lineWrap: boolean
  onToggleExpand: (index: number) => void
  hasBorderTop: boolean
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
  lineWrap,
  onToggleExpand,
  hasBorderTop,
}: LogRowProps) {
  const level = log.level
  const svcColorClass = `svc-color-${svcColorIndex}`

  const handleClick = useCallback(
    () => onToggleExpand(index),
    [onToggleExpand, index],
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
        className={cn(
          'log-line cursor-pointer flex',
          svcColorClass,
          hasBorderTop && 'border-t border-line-subtle',
          isActiveMatch && '!bg-accent/8',
          isExpanded && '!bg-surface-sunken',
          showServiceColumn && !levelTint && 'svc-row-tint',
          levelTint,
        )}
      >
        {/* Line number */}
        <span className="log-line-number py-[2px] pr-2 pl-1 shrink-0">
          {lineNumber}
        </span>

        {/* Timestamp */}
        <span className="log-ts pr-2 py-[2px] text-ink-tertiary select-none whitespace-nowrap tabular-nums text-[13px] font-mono shrink-0">
          {log.timestamp}
        </span>

        {/* Service color strip */}
        {showServiceColumn && (
          <span className={cn('w-[3px] min-h-full shrink-0', levelStrip || 'svc-strip-bg')} />
        )}

        {/* Service name — always visible, reduced weight for consecutive */}
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

        {/* Log content */}
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

      {isExpanded && <LogDetail log={log} svcColorClass={svcColorClass} />}
    </div>
  )
})
LogRow.displayName = 'LogRow'
