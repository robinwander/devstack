import { motion } from "framer-motion";
import { cn } from "@/lib/utils";
import type { RunStatusResponse } from "@/lib/api";

export function HealthSummary({ status, compact }: { status: RunStatusResponse; compact?: boolean }) {
  const entries = Object.entries(status.services);
  const ready = entries.filter(([, s]) => s.state === "ready").length;
  const failed = entries.filter(([, s]) => s.state === "failed" || s.state === "degraded").length;
  const starting = entries.filter(([, s]) => s.state === "starting").length;
  const total = entries.length;
  const allGood = ready === total;

  return (
    <div className="flex items-center gap-1.5 md:gap-2.5 ml-1.5 md:ml-3" title={entries.map(([name, s]) => `${name}: ${s.state}`).join("\n")}>
      <div className="flex items-center gap-0.5 md:gap-1" role="img" aria-label={`${ready} of ${total} services ready`}>
        {entries.map(([name, s], i) => (
          <motion.div
            key={name}
            initial={{ scaleY: 0 }}
            animate={{ scaleY: 1 }}
            transition={{ delay: i * 0.05, duration: 0.2, ease: "easeOut" }}
            className={cn(
              compact ? "w-1 h-3.5" : "w-1.5 h-5",
              "origin-bottom",
              s.state === "ready" && "bg-emerald-500",
              s.state === "starting" && "bg-amber-500",
              (s.state === "failed" || s.state === "degraded") && "bg-red-500",
              s.state === "stopped" && "bg-zinc-600",
            )}
            title={`${name}: ${s.state}`}
          />
        ))}
      </div>
      {!compact && (
        <span className={cn(
          "text-xs font-medium tabular-nums",
          allGood ? "text-emerald-400" : failed > 0 ? "text-red-400" : "text-amber-400"
        )}>
          {failed > 0 ? `${failed}/${total} failed` : `${ready}/${total} ready`}
          {starting > 0 && failed === 0 && <span className="text-muted-foreground/40 ml-1">starting</span>}
        </span>
      )}
    </div>
  );
}
