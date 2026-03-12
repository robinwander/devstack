import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { queries } from "@/lib/api";

export function DaemonStatus({ compact }: { compact?: boolean }) {
  const { data, isError, isLoading } = useQuery(queries.ping);
  const isConnected = data?.ok && !isError;

  return (
    <div
      className="flex items-center gap-1.5 px-2 h-8 text-xs font-medium shrink-0"
      role="status"
      aria-label={isLoading ? "Connecting to daemon" : isConnected ? "Daemon connected" : "Daemon offline"}
    >
      <span className={cn(
        "w-1.5 h-1.5 rounded-full shrink-0",
        isConnected ? "bg-status-green" : isLoading ? "bg-ink-tertiary pulse-dot" : "bg-status-red pulse-dot",
      )} />
      {!compact && (
        <span className={cn(
          "text-xs",
          isConnected ? "text-ink-tertiary" : isLoading ? "text-ink-tertiary" : "text-status-red-text",
        )}>
          {isLoading ? "Connecting…" : isConnected ? "Connected" : "Offline"}
        </span>
      )}
    </div>
  );
}

/** Error banner shown when daemon is unreachable */
export function DaemonBanner({ onRetry }: { onRetry: () => void }) {
  return (
    <div
      className="flex items-center justify-between gap-2 px-3 md:px-4 py-2 bg-status-red-tint border-b border-status-red text-xs md:text-sm text-status-red-text"
      role="alert"
    >
      <div className="flex items-center gap-2 min-w-0">
        <AlertTriangle className="w-4 h-4 shrink-0" />
        <span className="truncate">Cannot connect to daemon</span>
      </div>
      <button
        onClick={onRetry}
        className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-status-red-text hover:text-ink bg-status-red-tint border border-status-red rounded-md transition-colors shrink-0"
      >
        <RefreshCw className="w-3 h-3" />
        Retry
      </button>
    </div>
  );
}
