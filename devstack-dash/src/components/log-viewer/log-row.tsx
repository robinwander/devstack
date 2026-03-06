import { memo, useCallback } from "react";
import { cn } from "@/lib/utils";
import { highlightAll } from "./highlight";
import { LogDetail } from "./log-detail";
import type { ParsedLog } from "./types";

interface LogRowProps {
  log: ParsedLog;
  index: number;
  lineNumber: number;
  virtualRow: { index: number; start: number };
  measureElement: (el: Element | null) => void;
  showLabel: boolean;
  showServiceColumn: boolean;
  svcColorIndex: number;
  highlighter: string | RegExp | null;
  isActiveMatch: boolean;
  isExpanded: boolean;
  onToggleExpand: (index: number) => void;
  hasBorderTop: boolean;
}

export const LogRow = memo(function LogRow({
  log,
  index,
  lineNumber,
  virtualRow,
  measureElement,
  showLabel,
  showServiceColumn,
  svcColorIndex,
  highlighter,
  isActiveMatch,
  isExpanded,
  onToggleExpand,
  hasBorderTop,
}: LogRowProps) {
  const level = log.level;
  const svcColorClass = `svc-color-${svcColorIndex}`;

  const handleClick = useCallback(() => onToggleExpand(index), [onToggleExpand, index]);

  // Level tint and strip classes (14.5)
  const levelTint =
    level === "error"
      ? "log-level-error-tint"
      : level === "warn"
        ? "log-level-warn-tint"
        : "";
  const levelStrip =
    level === "error"
      ? "log-level-error-strip"
      : level === "warn"
        ? "log-level-warn-strip"
        : "";

  return (
    <div
      data-index={virtualRow.index}
      ref={measureElement}
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        width: "100%",
        transform: `translateY(${virtualRow.start}px)`,
      }}
    >
      <div
        onClick={handleClick}
        className={cn(
          "log-line cursor-pointer flex",
          svcColorClass,
          hasBorderTop && "border-t border-border/30",
          isActiveMatch && "!bg-primary/8",
          isExpanded && "!bg-secondary/40",
          // Service row tint in "All" tab (14.1)
          showServiceColumn && !levelTint && "svc-row-tint",
          // Level tint overrides service tint (14.5)
          levelTint,
        )}
      >
        {/* Line number (14.10) */}
        <span className="log-line-number py-[3px] pr-2 pl-1 shrink-0">
          {lineNumber}
        </span>

        {/* Timestamp */}
        <span className="log-ts pr-2 py-[3px] text-muted-foreground/40 select-none whitespace-nowrap tabular-nums text-[13px] shrink-0">
          {log.timestamp}
        </span>

        {/* Service color strip — 3px solid bar, always visible in All tab (14.1) */}
        {showServiceColumn && (
          <span className={cn("svc-strip shrink-0", levelStrip || "svc-strip-bg")} />
        )}

        {/* Service name — shown on service change, hidden for consecutive same-service */}
        {showServiceColumn && (
          <span
            className={cn(
              "py-[3px] px-2 select-none whitespace-nowrap font-semibold shrink-0 w-24 text-[13px]",
              showLabel ? "svc-text" : "text-transparent",
            )}
          >
            {log.service}
          </span>
        )}

        {/* Log content */}
        <span
          className={cn(
            "py-[3px] pl-2 pr-6 log-content-text min-w-0 flex-1",
            level === "error" && "text-red-400/90",
            level === "warn" && "text-amber-400/80",
            level === "info" && "text-foreground/65",
          )}
        >
          {log.json && (
            <span
              className="inline-block w-1.5 h-1.5 bg-primary/30 rounded-full mr-1.5 align-middle"
              title="JSON"
            />
          )}
          {highlighter ? highlightAll(log.content, highlighter) : log.content}
        </span>
      </div>

      {isExpanded && (
        <LogDetail
          log={log}
          svcColorClass={svcColorClass}
        />
      )}
    </div>
  );
});
LogRow.displayName = "LogRow";
