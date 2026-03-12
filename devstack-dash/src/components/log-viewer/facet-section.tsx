import { useState } from "react";
import { cn } from "@/lib/utils";
import type { FacetFilter } from "@/lib/api";

function facetValueTone(value: string): string {
  const lower = value.toLowerCase();
  if (lower === "error") return "text-status-red-text";
  if (lower === "warn" || lower === "warning") return "text-status-amber-text";
  return "text-ink-secondary";
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

  const displayName = filter.field.replace(/_/g, " ");

  return (
    <section className="px-3 py-2.5 border-b border-line-subtle" aria-label={`${displayName} filter`}>
      <div className="flex items-center justify-between mb-1.5">
        <span className="text-[11px] font-semibold tracking-wider uppercase text-ink-tertiary">
          {displayName}
        </span>
        {!isToggle && values.length > limit && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="text-[11px] text-ink-tertiary hover:text-ink transition-colors"
          >
            {expanded ? "Less" : "More"}
          </button>
        )}
      </div>

      {values.length === 0 ? (
        <div className="text-[11px] text-ink-tertiary">
          {loading ? (
            <div className="space-y-1.5">
              <div className="h-3 w-20 skeleton-shimmer" />
              <div className="h-3 w-14 skeleton-shimmer" />
            </div>
          ) : "—"}
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
                  "px-2 h-7 text-xs font-mono border rounded-sm transition-colors min-w-[32px]",
                  active
                    ? "bg-surface-sunken border-line"
                    : "border-line-subtle hover:bg-surface-sunken/50",
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
                  "text-xs font-mono rounded-sm transition-colors",
                  active ? "bg-surface-sunken" : "hover:bg-surface-sunken/50",
                )}
                title={value.value}
              >
                <div
                  className={cn("facet-bar", facetBarTone(value.value))}
                  style={{ width: `${barWidth}%` }}
                  aria-hidden="true"
                />
                <span className={cn("truncate relative", facetValueTone(value.value))}>
                  {value.value}
                </span>
                <span className="tabular-nums text-[11px] text-ink-tertiary shrink-0 relative">
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
