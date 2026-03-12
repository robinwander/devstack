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
  if (isAtLatest || hasSearch || newLogCount === 0) return null

  return (
    <button
      onClick={onScrollToLatest}
      className={cn(
        'absolute right-3 flex items-center gap-2 px-3 h-8 bg-surface-raised border border-line text-xs font-medium text-ink shadow-lg hover:bg-surface-sunken rounded-md transition-colors new-logs-toast',
        newestFirst ? 'top-3' : 'bottom-3',
      )}
    >
      <ArrowDown className={cn('w-3.5 h-3.5', newestFirst && 'rotate-180')} />
      {newestFirst ? '↑' : '↓'} {newLogCount} new lines
    </button>
  )
}
