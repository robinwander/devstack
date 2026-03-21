import { ArrowDown } from 'lucide-react'
import { cn } from '@/lib/utils'

interface LogScrollControlsProps {
  newLogCount: number
  isAtLatest: boolean
  hasSearch: boolean
  newestFirst: boolean
  onScrollToLatest: () => void
}

export function LogScrollControls({
  newLogCount,
  isAtLatest,
  hasSearch,
  newestFirst,
  onScrollToLatest,
}: LogScrollControlsProps) {
  const visible = !isAtLatest && !hasSearch && newLogCount > 0

  return (
    <div
      className={cn(
        'absolute right-3 z-20 transition-all',
        newestFirst ? 'top-3' : 'bottom-3',
        visible
          ? 'opacity-100 translate-y-0'
          : newestFirst
            ? 'opacity-0 -translate-y-2 pointer-events-none'
            : 'opacity-0 translate-y-2 pointer-events-none',
      )}
      style={{ transitionDuration: '200ms', transitionTimingFunction: 'cubic-bezier(0.16, 1, 0.3, 1)' }}
      aria-hidden={!visible}
    >
      <button
        onClick={onScrollToLatest}
        className="flex items-center gap-2 px-3 h-8 bg-surface-raised border border-line text-xs font-medium text-ink shadow-lg hover:bg-surface-sunken rounded-md transition-colors"
      >
        <ArrowDown className={cn('w-3.5 h-3.5', newestFirst && 'rotate-180')} />
        {newLogCount} new {newLogCount === 1 ? 'line' : 'lines'}
      </button>
    </div>
  )
}
