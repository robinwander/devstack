import { useMutation, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import {
  ChevronDown,
  Menu,
  X,
  MoreHorizontal,
  Square,
  XCircle,
  Trash2,
  Copy,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { DaemonStatus, EventStreamStatus } from './daemon-status'
import { LogoMark } from './logo'
import { StatusDot } from './status-dot'
import { HealthSummary } from './health-summary'
import {
  api,
  queryKeys,
  type ProjectSummary,
  type RunStatusResponse,
  type RunSummary,
  type SourceSummary,
} from '@/lib/api'

interface ActiveRun {
  run: RunSummary
  projectName: string
}

function runIdFragment(runId: string): string {
  return runId.slice(0, 8)
}

function relativeTime(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime()
  const mins = Math.floor(diff / 60000)
  if (mins < 1) return 'just now'
  if (mins < 60) return `${mins}m ago`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h ago`
  return `${Math.floor(hours / 24)}d ago`
}

export function Header({
  currentRun,
  currentProject,
  activeRuns,
  stoppedRuns,
  projects,
  sources,
  selectedSource,
  status,
  eventStreamConnected,
  onSelectRun,
  onSelectSource,
  isMobile,
  mobilePanelOpen,
  onToggleMobilePanel,
}: {
  currentRun: RunSummary | null
  currentProject: ProjectSummary | null
  activeRuns: ActiveRun[]
  stoppedRuns: RunSummary[]
  projects: ProjectSummary[]
  sources: SourceSummary[]
  selectedSource: string | null
  status: RunStatusResponse | undefined
  eventStreamConnected: boolean
  onSelectRun: (runId: string) => void
  onSelectSource: (name: string) => void
  isMobile?: boolean
  mobilePanelOpen?: boolean
  onToggleMobilePanel?: () => void
}) {
  const queryClient = useQueryClient()

  const gcMutation = useMutation({
    mutationFn: () => api.gc(undefined, true),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs })
      const total = data.removed_runs.length + data.removed_globals.length
      toast.success(total > 0 ? `Cleaned ${total} items` : 'Nothing to clean')
    },
    onError: (err) => toast.error(`Cleanup failed: ${err.message}`),
  })

  const downMutation = useMutation({
    mutationFn: () =>
      currentRun ? api.down(currentRun.run_id) : Promise.reject(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs })
      toast.success('Stack stopped')
    },
    onError: (err) => toast.error(`Failed to stop: ${err.message}`),
  })

  const killMutation = useMutation({
    mutationFn: () =>
      currentRun ? api.kill(currentRun.run_id) : Promise.reject(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs })
      toast.success('Stack killed')
    },
    onError: (err) => toast.error(`Failed to kill: ${err.message}`),
  })

  const copyRunId = () => {
    if (currentRun) {
      void navigator.clipboard.writeText(currentRun.run_id).then(
        () => toast.success('Run ID copied'),
        () => toast.error('Failed to copy'),
      )
    }
  }

  const isActive = currentRun && currentRun.state !== 'stopped'
  const showTargetSelector =
    !!currentRun ||
    !!selectedSource ||
    activeRuns.length > 0 ||
    stoppedRuns.length > 0 ||
    sources.length > 0

  return (
    <header className="relative z-10 flex items-center justify-between px-3 md:px-4 h-11 border-b border-line shrink-0 bg-surface-raised">
      <div className="flex items-center gap-2.5 md:gap-3 min-w-0">
        {/* Mobile hamburger */}
        {isMobile && currentRun && (
          <button
            onClick={onToggleMobilePanel}
            className="w-9 h-9 flex items-center justify-center text-ink-secondary hover:text-ink transition-colors -ml-1 shrink-0"
            aria-label={
              mobilePanelOpen ? 'Close services panel' : 'Open services panel'
            }
            aria-expanded={mobilePanelOpen}
          >
            {mobilePanelOpen ? (
              <X className="w-4 h-4" />
            ) : (
              <Menu className="w-4 h-4" />
            )}
          </button>
        )}

        {/* Wordmark */}
        <LogoMark />

        {showTargetSelector && (
          <>
            <div className="w-px h-4 bg-line mx-0.5 hidden sm:block" />

            {/* Run/source selector */}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button className="flex items-center gap-1.5 md:gap-2 px-2 md:px-2.5 h-8 text-sm font-medium rounded-md hover:bg-surface-sunken transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-ring min-w-0">
                  {selectedSource ? (
                    <span className="w-2 h-2 rounded-full bg-ink-tertiary shrink-0" />
                  ) : currentRun ? (
                    <StatusDot state={currentRun.state} />
                  ) : (
                    <span className="w-2 h-2 rounded-full bg-ink-tertiary shrink-0" />
                  )}
                  <span className="text-ink font-semibold truncate">
                    {selectedSource ?? currentRun?.stack ?? 'Select target'}
                  </span>
                  {!selectedSource && currentRun && (
                    <span
                      className="text-ink-tertiary text-xs hidden md:inline truncate max-w-[180px]"
                      title={currentProject?.name || currentRun.project_dir}
                    >
                      {currentProject?.name ||
                        currentRun.project_dir.split('/').pop()}
                    </span>
                  )}
                  <ChevronDown className="w-3 h-3 text-ink-tertiary shrink-0" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="start"
                className="w-80 max-w-[calc(100vw-2rem)]"
              >
                {activeRuns.length > 0 && (
                  <div className="px-3 py-2 text-[11px] font-semibold text-ink-tertiary uppercase tracking-wider">
                    Active
                  </div>
                )}
                {activeRuns.map(({ run, projectName }) => (
                  <DropdownMenuItem
                    key={run.run_id}
                    onClick={() => onSelectRun(run.run_id)}
                    className={cn(
                      'py-2 gap-2',
                      currentRun?.run_id === run.run_id &&
                        !selectedSource &&
                        'bg-secondary',
                    )}
                  >
                    <StatusDot state={run.state} />
                    <span className="font-medium">{run.stack}</span>
                    <span className="text-ink-tertiary text-xs truncate">
                      {projectName}
                    </span>
                    <span className="text-ink-tertiary text-[11px] font-mono ml-auto shrink-0">
                      {runIdFragment(run.run_id)}
                    </span>
                  </DropdownMenuItem>
                ))}
                {sources.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    <div className="px-3 py-2 text-[11px] font-semibold text-ink-tertiary uppercase tracking-wider">
                      Sources
                    </div>
                    {sources.map((source) => (
                      <DropdownMenuItem
                        key={source.name}
                        onClick={() => onSelectSource(source.name)}
                        className={cn(
                          'py-2 gap-2',
                          selectedSource === source.name && 'bg-secondary',
                        )}
                      >
                        <span className="w-2 h-2 rounded-full bg-ink-tertiary" />
                        <span className="font-medium">{source.name}</span>
                        <span className="text-ink-tertiary text-xs truncate ml-auto">
                          {source.paths.length} path
                          {source.paths.length !== 1 ? 's' : ''}
                        </span>
                      </DropdownMenuItem>
                    ))}
                  </>
                )}
                {stoppedRuns.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    <div className="px-3 py-2 text-[11px] font-semibold text-ink-tertiary uppercase tracking-wider">
                      Recent
                    </div>
                    {stoppedRuns.slice(0, 5).map((run) => {
                      const name =
                        projects.find((p) => p.path === run.project_dir)
                          ?.name || run.project_dir.split('/').pop()
                      return (
                        <DropdownMenuItem
                          key={run.run_id}
                          onClick={() => onSelectRun(run.run_id)}
                          className="py-2 gap-2"
                        >
                          <StatusDot state={run.state} />
                          <span>{run.stack}</span>
                          <span className="text-ink-tertiary text-xs truncate">
                            {name}
                          </span>
                          <span className="text-ink-tertiary text-[11px] ml-auto shrink-0">
                            {relativeTime(run.created_at)}
                          </span>
                        </DropdownMenuItem>
                      )
                    })}
                  </>
                )}
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  onClick={() => gcMutation.mutate()}
                  disabled={gcMutation.isPending}
                  className="py-2 gap-2"
                >
                  <Trash2 className="w-4 h-4" />
                  Clean up old runs
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>

            {/* Health summary */}
            {currentRun && status && (
              <HealthSummary status={status} compact={isMobile} />
            )}

            {/* Stack actions overflow — only when active */}
            {isActive && (
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    className="w-7 h-7 flex items-center justify-center text-ink-tertiary hover:text-ink-secondary hover:bg-surface-sunken rounded-md transition-colors"
                    aria-label="Stack actions"
                  >
                    <MoreHorizontal className="w-4 h-4" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-52">
                  <DropdownMenuItem onClick={copyRunId} className="gap-2">
                    <Copy className="w-3.5 h-3.5" />
                    Copy run ID
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    onClick={() => downMutation.mutate()}
                    disabled={downMutation.isPending}
                    className="gap-2"
                  >
                    <Square className="w-3.5 h-3.5" />
                    Stop stack
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onClick={() => {
                      if (
                        window.confirm(
                          'Force kill all processes in this stack?',
                        )
                      ) {
                        killMutation.mutate()
                      }
                    }}
                    disabled={killMutation.isPending}
                    className="gap-2 text-status-red-text"
                  >
                    <XCircle className="w-3.5 h-3.5" />
                    Force kill
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            )}
          </>
        )}
      </div>

      <div className="flex items-center gap-1">
        <EventStreamStatus connected={eventStreamConnected} compact={isMobile} />
        <DaemonStatus compact={isMobile} />
      </div>
    </header>
  )
}
