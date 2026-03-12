import { cn } from "@/lib/utils";
import type { RunStatusResponse } from "@/lib/api";
import { StatusDot } from "./status-dot";

export function HealthSummary({ status, compact }: { status: RunStatusResponse; compact?: boolean }) {
  const entries = Object.entries(status.services);
  const ready = entries.filter(([, s]) => s.state === "ready").length;
  const failed = entries.filter(([, s]) => s.state === "failed" || s.state === "degraded").length;
  const starting = entries.filter(([, s]) => s.state === "starting").length;
  const total = entries.length;

  const aggregateState = failed > 0 ? "failed" : starting > 0 ? "starting" : "ready";

  let label: string;
  if (failed > 0) {
    label = compact ? `${failed}✗` : `${failed}/${total} failed`;
  } else if (starting > 0) {
    label = compact ? `${ready}/${total}` : `${ready}/${total} starting`;
  } else {
    label = compact ? `${ready}/${total}` : `${ready}/${total} ready`;
  }

  return (
    <div
      className="flex items-center gap-1.5"
      title={entries.map(([name, s]) => `${name}: ${s.state}`).join("\n")}
      role="status"
      aria-label={`${ready} of ${total} services ready`}
    >
      <StatusDot state={aggregateState} size="sm" />
      <span className={cn(
        "text-xs font-medium tabular-nums",
        aggregateState === "ready" && "text-status-green-text",
        aggregateState === "starting" && "text-status-amber-text",
        aggregateState === "failed" && "text-status-red-text",
      )}>
        {label}
      </span>
    </div>
  );
}
