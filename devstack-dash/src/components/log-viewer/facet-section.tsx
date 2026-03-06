import { useState } from "react";
import { cn } from "@/lib/utils";
import type { FacetFilter } from "@/lib/api";

function facetValueTone(value: string): string {
  const lower = value.toLowerCase();
  if (lower === "error") return "text-red-400";
  if (lower === "warn" || lower === "warning") return "text-amber-400";
  return "text-foreground/70";
}

function facetBarTone(value: string): string {
  const lower = value.toLowerCase();
  if (lower === "error") return "facet-bar-error";
  if (lower === "warn" || lower === "warning") return "facet-bar-warn";
  return "";
}

export function FacetSection({
  filter,
  loading,
  onPick,
  isActive,
}: {
  filter: FacetFilter;
  loading?: boolean;
  onPick: (value: string) => void;
  isActive: (value: string) => boolean;
}) {
  const [expanded, setExpanded] = useState(false);
  const values = filter.values;
  const limit = 10;
  const shown = expanded ? values : values.slice(0, limit);
  const isToggle = filter.kind === "toggle";
  const maxCount = values.length > 0 ? Math.max(...values.map((v) => v.count)) : 0;

  return (
    <section className="px-3 py-2.5 border-b border-border/15" aria-label={`${filter.field} filter`}>
      {/* Group label — uppercase tracking-wider (14.14) */}
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[11px] font-semibold tracking-wider uppercase text-muted-foreground/50">
          {filter.field}
        </span>
        {!isToggle && values.length > limit && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="text-[11px] text-muted-foreground/35 hover:text-foreground transition-colors"
          >
            {expanded ? "Less" : "More"}
          </button>
        )}
      </div>

      {/* Empty state fix (13.2): show skeleton when loading, em dash when empty */}
      {values.length === 0 ? (
        <div className="text-[11px] text-muted-foreground/35">
          {loading ? (
            <div className="space-y-1.5">
              <div className="h-3 w-20 skeleton-shimmer" />
              <div className="h-3 w-14 skeleton-shimmer" />
            </div>
          ) : (
            "—"
          )}
        </div>
      ) : isToggle ? (
        <div className="flex flex-wrap gap-1.5">
          {values.map((value) => {
            const active = isActive(value.value);
            return (
              <button
                key={value.value}
                type="button"
                onClick={() => onPick(value.value)}
                aria-pressed={active}
                className={cn(
                  "px-2 h-7 text-xs font-mono border transition-colors min-w-[32px]",
                  active
                    ? "bg-secondary border-border"
                    : "border-border/40 hover:bg-secondary/40",
                )}
              >
                <span className={facetValueTone(value.value)}>{value.value}</span>
              </button>
            );
          })}
        </div>
      ) : (
        <div className="space-y-px">
          {shown.map((value) => {
            const active = isActive(value.value);
            const barWidth = maxCount > 0 ? (value.count / maxCount) * 100 : 0;
            return (
              <button
                key={value.value}
                type="button"
                onClick={() => onPick(value.value)}
                aria-pressed={active}
                className={cn(
                  "w-full flex items-center justify-between gap-3 px-2 py-1 relative",
                  "text-xs font-mono transition-colors",
                  active ? "bg-secondary/60" : "hover:bg-secondary/40",
                )}
                title={value.value}
              >
                {/* Proportional bar (14.11) */}
                <div
                  className={cn("facet-bar", facetBarTone(value.value))}
                  style={{ width: `${barWidth}%` }}
                  aria-hidden="true"
                />
                <span className={cn("truncate relative", facetValueTone(value.value))}>
                  {value.value}
                </span>
                <span className="tabular-nums text-[11px] text-muted-foreground/45 shrink-0 relative">
                  {value.count}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
}
