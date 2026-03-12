import { useCallback } from 'react'
import { cn } from '@/lib/utils'
import { ChevronsLeft, ChevronsRight } from 'lucide-react'
import { StatusDot } from './status-dot'
import { ServiceRow } from './service-row'
import {
  type RunStatusResponse,
  type RunSummary,
  type TaskExecutionSummary,
} from '@/lib/api'
import { getServiceColorIndex } from '@/lib/service-colors'

export function ServicePanel({
  run,
  status,
  globals,
  tasks,
  selectedService,
  selectedSource,
  onSelectService,
  isMobile,
  mobilePanelOpen,
  onCloseMobilePanel,
  collapsed,
  onToggleCollapse,
}: {
  run: RunSummary
  status: RunStatusResponse
  globals: {
    key: string
    name: string
    state: string
    port: number | null
    url: string | null
  }[]
  tasks: TaskExecutionSummary[]
  selectedService: string | null
  selectedSource?: string | null
  onSelectService: (name: string | null) => void
  isMobile?: boolean
  mobilePanelOpen?: boolean
  onCloseMobilePanel?: () => void
  collapsed?: boolean
  onToggleCollapse?: () => void
}) {
  void onCloseMobilePanel
  void selectedSource
  const serviceEntries = Object.entries(status.services)

  const handleSelectService = useCallback(
    (name: string) => onSelectService(selectedService === name ? null : name),
    [onSelectService, selectedService],
  )

  const panelOpen = isMobile ? mobilePanelOpen : true

  return (
    <aside
      className={cn(
        'flex flex-col shrink-0 border-r border-line sidebar-transition',
        isMobile
          ? 'fixed inset-y-0 left-0 z-30 w-[240px] mobile-slide-panel shadow-lg bg-surface-raised'
          : collapsed
            ? 'w-10 bg-surface-raised'
            : 'w-[160px] bg-surface-raised',
        isMobile && !panelOpen && '-translate-x-full',
        isMobile && panelOpen && 'translate-x-0',
      )}
    >
      {/* Section header */}
      <div
        className={cn(
          'flex items-center border-b border-line shrink-0',
          collapsed ? 'justify-center py-2' : 'justify-between px-3 py-2',
        )}
      >
        {!collapsed && (
          <span className="text-[11px] font-semibold text-ink-tertiary uppercase tracking-wider">
            Services
          </span>
        )}
        {!isMobile && onToggleCollapse && (
          <button
            onClick={onToggleCollapse}
            className="w-6 h-6 flex items-center justify-center text-ink-tertiary hover:text-ink transition-colors rounded-sm"
            title={collapsed ? 'Expand sidebar (])' : 'Collapse sidebar ([)'}
            aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
          >
            {collapsed ? (
              <ChevronsRight className="w-3.5 h-3.5" />
            ) : (
              <ChevronsLeft className="w-3.5 h-3.5" />
            )}
          </button>
        )}
      </div>

      {/* Services list */}
      <nav className="flex-1 overflow-y-auto py-1" aria-label="Services">
        {collapsed ? (
          /* Collapsed: just status dots */
          <>
            {serviceEntries.map(([name, svc]) => (
              <button
                key={name}
                onClick={() => handleSelectService(name)}
                className={cn(
                  'w-full flex items-center justify-center py-2 transition-colors',
                  selectedService === name
                    ? 'bg-surface-sunken'
                    : 'hover:bg-surface-sunken/50',
                )}
                title={`${name} — ${svc.state}`}
                aria-label={`${name} — ${svc.state}`}
              >
                <StatusDot state={svc.state} />
              </button>
            ))}
            {tasks.length > 0 && (
              <div className="border-t border-line-subtle mt-1 pt-1">
                {tasks.map((task) => {
                  const taskKey = `task:${task.task}`
                  return (
                    <button
                      key={task.task}
                      onClick={() => handleSelectService(taskKey)}
                      className={cn(
                        'w-full flex items-center justify-center py-2 transition-colors',
                        selectedService === taskKey
                          ? 'bg-surface-sunken'
                          : 'hover:bg-surface-sunken/50',
                      )}
                      title={`${task.task} — ${task.exit_code === 0 ? 'passed' : 'failed'}`}
                      aria-label={`${task.task} — ${task.exit_code === 0 ? 'passed' : 'failed'}`}
                    >
                      <span
                        className={cn(
                          'text-xs',
                          task.exit_code === 0
                            ? 'text-emerald-500'
                            : 'text-red-400',
                        )}
                      >
                        {task.exit_code === 0 ? '✓' : '✗'}
                      </span>
                    </button>
                  )
                })}
              </div>
            )}
            {globals.length > 0 && (
              <div className="border-t border-line-subtle mt-1 pt-1">
                {globals.map((g) => (
                  <div
                    key={g.key}
                    className="flex items-center justify-center py-2"
                    title={`${g.name} — ${g.state}`}
                  >
                    <StatusDot
                      state={g.state === 'running' ? 'running' : 'stopped'}
                    />
                  </div>
                ))}
              </div>
            )}
          </>
        ) : (
          /* Expanded: full rows */
          <>
            {serviceEntries.map(([name, svc]) => (
              <ServiceRow
                key={name}
                name={name}
                service={svc}
                isViewing={selectedService === name}
                onSelect={handleSelectService}
                svcColorIndex={getServiceColorIndex(name)}
              />
            ))}
            {tasks.length > 0 && (
              <div className="mt-1 pt-1 border-t border-line-subtle">
                <div className="text-[11px] font-semibold text-ink-tertiary uppercase tracking-wider px-3 pb-1 pt-1.5">
                  Tasks
                </div>
                {tasks.map((task) => {
                  const taskKey = `task:${task.task}`
                  return (
                    <button
                      key={task.task}
                      onClick={() => onSelectService(taskKey)}
                      className={cn(
                        'w-full flex items-center gap-2 px-3 py-1.5 text-[13px] transition-colors',
                        selectedService === taskKey
                          ? 'bg-surface-sunken'
                          : 'hover:bg-surface-sunken/50',
                      )}
                    >
                      <span
                        className={cn(
                          'text-xs',
                          task.exit_code === 0
                            ? 'text-emerald-500'
                            : 'text-red-400',
                        )}
                      >
                        {task.exit_code === 0 ? '✓' : '✗'}
                      </span>
                      <span className="text-ink-secondary truncate">
                        {task.task}
                      </span>
                      <span className="text-ink-tertiary text-[11px] ml-auto">
                        {(task.duration_ms / 1000).toFixed(1)}s
                      </span>
                    </button>
                  )
                })}
              </div>
            )}
            {globals.length > 0 && (
              <div className="mt-1 pt-1 border-t border-line-subtle">
                <div className="text-[11px] font-semibold text-ink-tertiary uppercase tracking-wider px-3 pb-1 pt-1.5">
                  Globals
                </div>
                {globals.map((g) => (
                  <div
                    key={g.key}
                    className="flex items-center gap-2 px-3 py-1.5 text-[13px]"
                  >
                    <StatusDot
                      state={g.state === 'running' ? 'running' : 'stopped'}
                    />
                    <span className="text-ink-secondary truncate">
                      {g.name}
                    </span>
                    {g.port && (
                      <span className="text-ink-tertiary font-mono text-[11px] ml-auto">
                        :{g.port}
                      </span>
                    )}
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </nav>

      {/* Project path — hidden when collapsed */}
      {!collapsed && (
        <div className="px-3 py-2 border-t border-line-subtle">
          <div
            className="text-[11px] text-ink-tertiary font-mono truncate"
            title={run.project_dir}
          >
            {run.project_dir}
          </div>
        </div>
      )}
    </aside>
  )
}
