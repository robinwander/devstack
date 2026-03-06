import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { queries } from "@/lib/api";

export function DaemonStatus({ compact }: { compact?: boolean }) {
  const { data, isError, isLoading } = useQuery(queries.ping);
  const isConnected = data?.ok && !isError;

  return (
    <div
      className="flex items-center gap-2 px-2 md:px-3 h-9 text-xs font-medium shrink-0"
      role="status"
      aria-label={isLoading ? "Connecting to daemon" : isConnected ? "Daemon connected" : "Daemon offline"}
    >
      <span className={cn(
        "w-1.5 h-1.5 rounded-full shrink-0",
        isConnected ? "bg-emerald-400 status-glow-green" : isLoading ? "bg-muted-foreground/40 pulse-dot" : "bg-red-400 status-glow-red pulse-dot",
      )} />
      {!compact && (
        <span className={cn(
          isConnected ? "text-muted-foreground/50" : isLoading ? "text-muted-foreground/40" : "text-red-400/70",
        )}>
          {isLoading ? "Connecting…" : isConnected ? "Daemon" : "Offline"}
        </span>
      )}
    </div>
  );
}

/** Error banner shown when daemon is unreachable (7.2) */
export function DaemonBanner({ onRetry }: { onRetry: () => void }) {
  return (
    <div
      className="flex items-center justify-between gap-2 px-3 md:px-5 py-2 bg-red-500/10 border-b border-red-500/20 text-xs md:text-sm text-red-400"
      role="alert"
    >
      <div className="flex items-center gap-2 min-w-0">
        <AlertTriangle className="w-4 h-4 shrink-0" />
        <span className="truncate">Cannot connect to daemon</span>
      </div>
      <button
        onClick={onRetry}
        className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-red-300 hover:text-foreground bg-red-500/10 hover:bg-red-500/20 border border-red-500/20 transition-colors shrink-0"
      >
        <RefreshCw className="w-3 h-3" />
        Retry
      </button>
    </div>
  );
}
