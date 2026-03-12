import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'
import { StatusDot } from '@/components/status-dot'
import type { RunStatusResponse } from '@/lib/api'

interface LogTabBarProps {
  services: string[]
  activeTab: string
  status: RunStatusResponse | undefined
  onSelectService: (name: string | null) => void
  children?: ReactNode
}

export function LogTabBar({
  services,
  activeTab,
  status,
  onSelectService,
  children,
}: LogTabBarProps) {
  const allStates = status
    ? Object.values(status.services).map((s) => s.state)
    : []
  const hasAnyFailed = allStates.some((s) => s === 'failed' || s === 'degraded')
  const hasAnyStarting = allStates.some((s) => s === 'starting')
  const allState = hasAnyFailed
    ? 'failed'
    : hasAnyStarting
      ? 'starting'
      : 'ready'
  const hasExtraTab = activeTab !== '__all__' && !services.includes(activeTab)
  const extraTabLabel = activeTab.startsWith('task:')
    ? activeTab.slice(5)
    : activeTab

  return (
    <div className="flex items-center justify-between px-2 md:px-3 gap-2 md:gap-4 min-w-0">
      <div
        className="flex items-center overflow-x-auto scrollbar-none min-w-0"
        role="tablist"
        aria-label="Service log tabs"
      >
        <TabButton
          active={activeTab === '__all__'}
          onClick={() => onSelectService(null)}
        >
          {status && <StatusDot state={allState} size="sm" />}
          All
        </TabButton>
        {services.map((svc) => {
          const svcState = status?.services[svc]?.state ?? 'stopped'
          return (
            <TabButton
              key={svc}
              active={activeTab === svc}
              onClick={() => onSelectService(svc)}
            >
              {status && <StatusDot state={svcState} size="sm" />}
              {svc}
            </TabButton>
          )
        })}
        {hasExtraTab && (
          <TabButton active onClick={() => onSelectService(activeTab)}>
            {extraTabLabel}
          </TabButton>
        )}
      </div>
      {children}
    </div>
  )
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: ReactNode
}) {
  return (
    <button
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={cn(
        'relative px-3 md:px-3.5 h-9 text-[13px] transition-colors shrink-0 flex items-center gap-1.5',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-ring rounded-sm',
        active
          ? 'text-ink font-semibold tab-active'
          : 'text-ink-tertiary hover:text-ink-secondary',
      )}
    >
      {children}
    </button>
  )
}
