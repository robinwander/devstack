import { useCallback } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import {
  Square,
  XCircle,
  Activity,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { StatusDot } from "./status-dot";
import { ServiceRow } from "./service-row";
import {
  api,
  queryKeys,
  type RunSummary,
  type RunStatusResponse,
} from "@/lib/api";

export function ServicePanel({
  run,
  status,
  globals,
  selectedService,
  onSelectService,
  isMobile,
  mobilePanelOpen,
  onCloseMobilePanel,
}: {
  run: RunSummary;
  status: RunStatusResponse;
  globals: { key: string; name: string; state: string; port: number | null; url: string | null }[];
  selectedService: string | null;
  onSelectService: (name: string | null) => void;
  isMobile?: boolean;
  mobilePanelOpen?: boolean;
  onCloseMobilePanel?: () => void;
}) {
  void onCloseMobilePanel; // Used by parent for backdrop clicks
  const queryClient = useQueryClient();
  const serviceEntries = Object.entries(status.services);

  const downMutation = useMutation({
    mutationFn: () => api.down(run.run_id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs });
      toast.success("Stack stopped");
    },
    onError: (err) => toast.error(`Failed to stop stack: ${err.message}`),
  });

  const killMutation = useMutation({
    mutationFn: () => api.kill(run.run_id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs });
      toast.success("Stack killed");
    },
    onError: (err) => toast.error(`Failed to kill stack: ${err.message}`),
  });

  const restartMutation = useMutation({
    mutationFn: (service: string) => api.restartService(run.run_id, service),
    onSuccess: (_, service) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runStatus(run.run_id) });
      toast.success(`Restarted ${service}`);
    },
    onError: (err) => toast.error(`Failed to restart: ${err.message}`),
  });

  // Stable callbacks to avoid re-rendering ServiceRow on every poll (Bug 3)
  const handleSelectService = useCallback(
    (name: string) => onSelectService(selectedService === name ? null : name),
    [onSelectService, selectedService],
  );

  const handleRestart = useCallback(
    (name: string) => restartMutation.mutate(name),
    [restartMutation],
  );

  const isActive = run.state !== "stopped";

  const panelOpen = isMobile ? mobilePanelOpen : true;

  return (
    <aside
      className={cn(
        "flex flex-col shrink-0 border-r border-border",
        // Mobile: fixed slide-over panel with opaque bg
        isMobile
          ? "fixed inset-y-0 left-0 z-30 w-[280px] mobile-slide-panel shadow-2xl bg-background"
          : "w-[260px] bg-card/50",
        // Transform for mobile slide
        isMobile && !panelOpen && "-translate-x-full",
        isMobile && panelOpen && "translate-x-0",
      )}
    >
      {/* Stack controls */}
      <div className="px-4 py-3 border-b border-border flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 min-w-0">
          <Activity className="w-3.5 h-3.5 text-muted-foreground/40 shrink-0" />
          <span className="text-xs text-muted-foreground/60 font-mono truncate" title={run.run_id}>
            {run.run_id}
          </span>
        </div>
        {isActive && (
          <div className="flex items-center gap-1 md:gap-0.5 shrink-0">
            <Button variant="ghost" size={isMobile ? "icon-sm" : "icon-xs"} onClick={() => downMutation.mutate()} disabled={downMutation.isPending} aria-label="Stop stack" title="Stop stack">
              <Square className="w-3.5 h-3.5" />
            </Button>
            <Button variant="ghost" size={isMobile ? "icon-sm" : "icon-xs"} onClick={() => killMutation.mutate()} disabled={killMutation.isPending} aria-label="Kill stack" title="Kill stack" className="text-destructive hover:text-destructive">
              <XCircle className="w-3.5 h-3.5" />
            </Button>
          </div>
        )}
      </div>

      {/* Services list */}
      <nav className="flex-1 overflow-y-auto py-1.5 stagger-in" aria-label="Services">
        {serviceEntries.map(([name, svc]) => (
          <ServiceRow
            key={name}
            name={name}
            service={svc}
            isViewing={selectedService === name}
            onSelect={handleSelectService}
            onRestart={handleRestart}
            isRestarting={restartMutation.isPending && restartMutation.variables === name}
          />
        ))}

        {globals.length > 0 && (
          <div className="mt-2 pt-2 border-t border-border/50">
            <div className="text-[11px] font-semibold text-muted-foreground/40 uppercase tracking-wider px-4 pb-1.5">
              Globals
            </div>
            {globals.map((g) => (
              <div key={g.key} className="flex items-center gap-2.5 px-4 py-2 text-sm">
                <StatusDot state={g.state === "running" ? "running" : "stopped"} />
                <span className="text-foreground/50 truncate">{g.name}</span>
                {g.port && (
                  <span className="text-muted-foreground/50 font-mono text-xs ml-auto">:{g.port}</span>
                )}
              </div>
            ))}
          </div>
        )}
      </nav>

      {/* Project path */}
      <div className="px-4 py-2.5 border-t border-border/50">
        <div className="text-[11px] text-muted-foreground/40 font-mono truncate" title={run.project_dir}>
          {run.project_dir}
        </div>
      </div>
    </aside>
  );
}
