import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { StatusDot } from "@/components/status-dot";
import type { RunStatusResponse } from "@/lib/api";

interface LogTabBarProps {
  services: string[];
  activeTab: string;
  status: RunStatusResponse | undefined;
  onSelectService: (name: string | null) => void;
  children?: ReactNode;
}

export function LogTabBar({ services, activeTab, status, onSelectService, children }: LogTabBarProps) {
  // Compute aggregate health for "All" tab
  const allStates = status ? Object.values(status.services).map((s) => s.state) : [];
  const hasAnyFailed = allStates.some((s) => s === "failed" || s === "degraded");
  const hasAnyStarting = allStates.some((s) => s === "starting");
  const allState = hasAnyFailed ? "failed" : hasAnyStarting ? "starting" : "ready";

  return (
    <div className="flex items-center justify-between px-2 md:px-3 gap-2 md:gap-4">
      <div className="flex items-center overflow-x-auto scrollbar-none" role="tablist" aria-label="Service log tabs">
        <TabButton active={activeTab === "__all__"} onClick={() => onSelectService(null)}>
          {status && <StatusDot state={allState} size="sm" />}
          All
        </TabButton>
        {services.map((svc) => {
          const svcState = status?.services[svc]?.state ?? "stopped";
          return (
            <TabButton key={svc} active={activeTab === svc} onClick={() => onSelectService(svc)}>
              {status && <StatusDot state={svcState} size="sm" />}
              {svc}
            </TabButton>
          );
        })}
      </div>

      {/* Right-side controls (passed as children) */}
      {children}
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={cn(
        "relative px-3 md:px-4 h-10 text-sm transition-colors shrink-0 flex items-center gap-1.5",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
        active
          ? "text-foreground font-semibold tab-active"
          : "text-muted-foreground/50 hover:text-foreground/80",
      )}
    >
      {children}
    </button>
  );
}
