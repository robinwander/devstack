import { memo, useCallback, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { ColumnConfig } from '@/lib/column-detection'
import { ColumnPicker } from './column-picker'

export interface ColumnSort {
  field: string
  direction: 'asc' | 'desc'
}

interface LogTableHeaderProps {
  columns: ColumnConfig[]
  showServiceColumn: boolean
  serviceColumnWidth: number
  lineWrap: boolean
  attributeCardinality: Map<string, number>
  columnSort: ColumnSort | null
  isMobile?: boolean
  onToggleColumn: (field: string) => void
  onRemoveColumn: (field: string) => void
  onResizeColumn: (field: string, width: number) => void
  onColumnSort: (field: string) => void
}

export const LogTableHeader = memo(function LogTableHeader({
  columns,
  showServiceColumn,
  serviceColumnWidth,
  lineWrap,
  attributeCardinality,
  columnSort,
  isMobile,
  onToggleColumn,
  onRemoveColumn,
  onResizeColumn,
  onColumnSort,
}: LogTableHeaderProps) {
  const visibleDynamic = columns.filter((c) => c.visible && !c.builtIn)

  const timeSortIndicator =
    columnSort?.field === '__time__'
      ? columnSort.direction === 'asc'
        ? ' ▲'
        : ' ▼'
      : ''

  return (
    <div className="log-table-header flex items-stretch border-b border-line bg-surface-raised sticky top-0 z-20 min-w-0 select-none">
      {/* Line number */}
      <div className="log-col-header log-line-number-header shrink-0" style={{ width: isMobile ? 32 : 44 }}>
        <span className="text-[10px] text-ink-tertiary">#</span>
      </div>

      {/* Timestamp */}
      <div
        className="log-col-header shrink-0 cursor-pointer hover:bg-surface-sunken/50 transition-colors"
        style={{ width: isMobile ? 80 : 108 }}
        onClick={() => onColumnSort('__time__')}
        title="Sort by time"
      >
        <span className={cn('log-col-label', columnSort?.field === '__time__' && 'text-accent')}>
          time{timeSortIndicator}
        </span>
      </div>

      {/* Service */}
      {showServiceColumn && (
        <>
          {/* Color strip placeholder */}
          <div className="w-[3px] shrink-0" />
          <div
            className="log-col-header shrink-0"
            style={{ width: serviceColumnWidth }}
          >
            <span className="log-col-label">service</span>
          </div>
        </>
      )}

      {/* Level */}
      <div className="log-col-header shrink-0" style={{ width: 56 }}>
        <span className="log-col-label">level</span>
      </div>

      {/* Dynamic attribute columns — hidden on mobile */}
      {!isMobile &&
        visibleDynamic.map((col) => (
          <ResizableColumnHeader
            key={col.field}
            column={col}
            columnSort={columnSort}
            onRemove={onRemoveColumn}
            onResize={onResizeColumn}
            onSort={onColumnSort}
          />
        ))}

      {/* Message — flex-grows */}
      <div
        className={cn(
          'log-col-header flex-1 min-w-0',
          lineWrap ? 'pr-2' : '',
        )}
      >
        <span className="log-col-label">message</span>
      </div>

      {/* Add column button — hidden on mobile */}
      {!isMobile && (
        <ColumnPicker
          columns={columns}
          attributeCardinality={attributeCardinality}
          onToggleColumn={onToggleColumn}
        />
      )}
    </div>
  )
})

function ResizableColumnHeader({
  column,
  columnSort,
  onRemove,
  onResize,
  onSort,
}: {
  column: ColumnConfig
  columnSort: ColumnSort | null
  onRemove: (field: string) => void
  onResize: (field: string, width: number) => void
  onSort: (field: string) => void
}) {
  const [isHovered, setIsHovered] = useState(false)
  const headerRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<{
    startX: number
    startWidth: number
  } | null>(null)

  const isActiveSortField = columnSort?.field === column.field

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()

      const startX = e.clientX
      const startWidth = column.width

      dragRef.current = { startX, startWidth }

      const handleMouseMove = (moveEvent: MouseEvent) => {
        if (!dragRef.current) return
        const delta = moveEvent.clientX - dragRef.current.startX
        const newWidth = Math.max(64, Math.min(400, dragRef.current.startWidth + delta))
        onResize(column.field, newWidth)
      }

      const handleMouseUp = () => {
        dragRef.current = null
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
      }

      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
    },
    [column.field, column.width, onResize],
  )

  const handleRemove = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation()
      onRemove(column.field)
    },
    [column.field, onRemove],
  )

  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      onRemove(column.field)
    },
    [column.field, onRemove],
  )

  const handleClick = useCallback(() => {
    onSort(column.field)
  }, [column.field, onSort])

  const sortIndicator = isActiveSortField
    ? columnSort?.direction === 'asc'
      ? ' ▲'
      : ' ▼'
    : ''

  return (
    <div
      ref={headerRef}
      className="log-col-header shrink-0 relative group cursor-pointer hover:bg-surface-sunken/50 transition-colors"
      style={{ width: column.width }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      onContextMenu={handleContextMenu}
      onClick={handleClick}
      title={`Click to sort by ${column.label} · Right-click to remove`}
    >
      <span className={cn('log-col-label truncate', isActiveSortField && 'text-accent')}>
        {column.label}{sortIndicator}
      </span>
      {isHovered && (
        <button
          onClick={handleRemove}
          className="absolute right-5 top-1/2 -translate-y-1/2 w-4 h-4 flex items-center justify-center rounded-sm text-ink-tertiary hover:text-ink hover:bg-surface-sunken transition-colors"
          aria-label={`Remove ${column.label} column`}
        >
          <X className="w-3 h-3" />
        </button>
      )}
      {/* Resize handle */}
      <div
        className="absolute right-0 top-0 bottom-0 w-[5px] cursor-col-resize hover:bg-accent/20 transition-colors"
        onMouseDown={handleMouseDown}
        aria-hidden="true"
      />
    </div>
  )
}
