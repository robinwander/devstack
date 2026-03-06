import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Trash2, ChevronDown, Menu, X } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { DaemonStatus } from "./daemon-status";
import { LogoMark } from "./logo";
import { StatusDot } from "./status-dot";
import { HealthSummary } from "./health-summary";
import {
  api,
  queryKeys,
  type RunSummary,
  type RunStatusResponse,
  type ProjectSummary,
} from "@/lib/api";

interface ActiveRun {
  run: RunSummary;
  projectName: string;
}

function runIdFragment(runId: string): string {
  return runId.slice(0, 8);
}

function relativeTime(dateStr: string): string {
  const diff = Date.now() - new Date(dateStr).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

export function Header({
  currentRun,
  currentProject,
  activeRuns,
  stoppedRuns,
  projects,
  status,
  onSelectRun,
  isMobile,
  mobilePanelOpen,
  onToggleMobilePanel,
}: {
  currentRun: RunSummary | null;
  currentProject: ProjectSummary | null;
  activeRuns: ActiveRun[];
  stoppedRuns: RunSummary[];
  projects: ProjectSummary[];
  status: RunStatusResponse | undefined;
  onSelectRun: (runId: string) => void;
  isMobile?: boolean;
  mobilePanelOpen?: boolean;
  onToggleMobilePanel?: () => void;
}) {
  const queryClient = useQueryClient();

  const gcMutation = useMutation({
    mutationFn: () => api.gc(undefined, true),
    onSuccess: (data) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.runs });
      const total = data.removed_runs.length + data.removed_globals.length;
      toast.success(total > 0 ? `Cleaned ${total} items` : "Nothing to clean");
    },
    onError: (err) => toast.error(`Cleanup failed: ${err.message}`),
  });

  return (
    <header className="relative z-10 flex items-center justify-between px-3 md:px-5 h-12 border-b border-border shrink-0">
      <div className="absolute inset-0 header-glow pointer-events-none" />
      <div className="relative flex items-center gap-2 md:gap-3 min-w-0">
        {/* Mobile menu toggle */}
        {isMobile && currentRun && (
          <button
            onClick={onToggleMobilePanel}
            className="w-10 h-10 flex items-center justify-center text-muted-foreground hover:text-foreground transition-colors -ml-1 shrink-0"
            aria-label={mobilePanelOpen ? "Close services panel" : "Open services panel"}
            aria-expanded={mobilePanelOpen}
          >
            {mobilePanelOpen ? <X className="w-5 h-5" /> : <Menu className="w-5 h-5" />}
          </button>
        )}

        {/* Logo mark */}
        <div className="flex items-center gap-2 md:gap-2.5 shrink-0">
          <LogoMark />
          <span className="text-[11px] font-semibold tracking-widest text-muted-foreground uppercase select-none hidden sm:inline">
            devstack
          </span>
        </div>

        {currentRun && (
          <>
            <div className="w-px h-5 bg-border mx-0.5 md:mx-1 hidden sm:block" />
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <button className="flex items-center gap-1.5 md:gap-2.5 px-2 md:px-3 h-9 text-sm font-medium border border-border hover:bg-secondary/50 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring min-w-0">
                  <StatusDot state={currentRun.state} />
                  <span className="text-foreground font-semibold truncate">{currentRun.stack}</span>
                  <span className="text-muted-foreground/50 text-xs hidden md:inline truncate max-w-[180px]" title={currentProject?.name || currentRun.project_dir}>
                    {currentProject?.name || currentRun.project_dir.split("/").pop()}
                  </span>
                  <ChevronDown className="w-3.5 h-3.5 text-muted-foreground/60 shrink-0" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start" className="w-80 max-w-[calc(100vw-2rem)]">
                {activeRuns.length > 0 && (
                  <div className="px-3 py-2 text-[11px] font-semibold text-muted-foreground/50 uppercase tracking-wider">Active</div>
                )}
                {activeRuns.map(({ run, projectName }) => (
                  <DropdownMenuItem key={run.run_id} onClick={() => onSelectRun(run.run_id)} className={cn("py-2.5 gap-2.5", currentRun?.run_id === run.run_id && "bg-secondary")}>
                    <StatusDot state={run.state} />
                    <span className="font-medium">{run.stack}</span>
                    <span className="text-muted-foreground text-xs truncate">{projectName}</span>
                    <span className="text-muted-foreground/60 text-[11px] font-mono ml-auto shrink-0">{runIdFragment(run.run_id)}</span>
                  </DropdownMenuItem>
                ))}
                {stoppedRuns.length > 0 && (
                  <>
                    <DropdownMenuSeparator />
                    <div className="px-3 py-2 text-[11px] font-semibold text-muted-foreground/50 uppercase tracking-wider">Recent</div>
                    {stoppedRuns.slice(0, 5).map((run) => {
                      const name = projects.find((p) => p.path === run.project_dir)?.name || run.project_dir.split("/").pop();
                      return (
                        <DropdownMenuItem key={run.run_id} onClick={() => onSelectRun(run.run_id)} className="py-2.5 gap-2.5">
                          <StatusDot state={run.state} />
                          <span>{run.stack}</span>
                          <span className="text-muted-foreground text-xs truncate">{name}</span>
                          <span className="text-muted-foreground/40 text-[11px] ml-auto shrink-0">{relativeTime(run.created_at)}</span>
                        </DropdownMenuItem>
                      );
                    })}
                  </>
                )}
                <DropdownMenuSeparator />
                <DropdownMenuItem onClick={() => gcMutation.mutate()} disabled={gcMutation.isPending} className="py-2.5 gap-2.5">
                  <Trash2 className="w-4 h-4" />
                  Clean up old runs
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>

            {/* Health summary — hide the text on very small screens */}
            {status && <HealthSummary status={status} compact={isMobile} />}
          </>
        )}
      </div>

      <DaemonStatus compact={isMobile} />
    </header>
  );
}
