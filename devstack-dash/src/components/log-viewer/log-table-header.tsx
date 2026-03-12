import { memo, useCallback, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { ColumnConfig } from '@/lib/column-detection'
import { ColumnPicker } from './column-picker'

interface LogTableHeaderProps {
  columns: ColumnConfig[]
  showServiceColumn: boolean
  serviceColumnWidth: number
  lineWrap: boolean
  attributeCardinality: Map<string, number>
  onToggleColumn: (field: string) => void
  onRemoveColumn: (field: string) => void
  onResizeColumn: (field: string, width: number) => void
}

export const LogTableHeader = memo(function LogTableHeader({
  columns,
  showServiceColumn,
  serviceColumnWidth,
  lineWrap,
  attributeCardinality,
  onToggleColumn,
  onRemoveColumn,
  onResizeColumn,
}: LogTableHeaderProps) {
  const visibleDynamic = columns.filter((c) => c.visible && !c.builtIn)

  return (
    <div className="log-table-header flex items-stretch border-b border-line bg-surface-raised sticky top-0 z-20 min-w-0 select-none">
      {/* Line number */}
      <div className="log-col-header log-line-number-header shrink-0" style={{ width: 44 }}>
        <span className="text-[10px] text-ink-tertiary">#</span>
      </div>

      {/* Timestamp */}
      <div className="log-col-header shrink-0" style={{ width: 108 }}>
        <span className="log-col-label">time</span>
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

      {/* Dynamic attribute columns */}
      {visibleDynamic.map((col) => (
        <ResizableColumnHeader
          key={col.field}
          column={col}
          onRemove={onRemoveColumn}
          onResize={onResizeColumn}
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

      {/* Add column button */}
      <ColumnPicker
        columns={columns}
        attributeCardinality={attributeCardinality}
        onToggleColumn={onToggleColumn}
      />
    </div>
  )
})

function ResizableColumnHeader({
  column,
  onRemove,
  onResize,
}: {
  column: ColumnConfig
  onRemove: (field: string) => void
  onResize: (field: string, width: number) => void
}) {
  const [isHovered, setIsHovered] = useState(false)
  const headerRef = useRef<HTMLDivElement>(null)
  const dragRef = useRef<{
    startX: number
    startWidth: number
  } | null>(null)

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

  return (
    <div
      ref={headerRef}
      className="log-col-header shrink-0 relative group"
      style={{ width: column.width }}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
    >
      <span className="log-col-label truncate">{column.label}</span>
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
