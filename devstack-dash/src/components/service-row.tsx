import { useCallback, useState, useRef } from "react";
import { createPortal } from "react-dom";
import { cn } from "@/lib/utils";
import { Copy, ExternalLink, Check } from "lucide-react";
import { StatusDot } from "./status-dot";
import type { ServiceStatus } from "@/lib/api";

export function ServiceRow({
  name,
  service,
  isViewing,
  onSelect,
  svcColorIndex,
}: {
  name: string;
  service: ServiceStatus;
  isViewing: boolean;
  onSelect: (name: string) => void;
  svcColorIndex: number;
}) {
  const handleSelect = useCallback(() => onSelect(name), [onSelect, name]);
  const svcColorClass = `svc-color-${svcColorIndex}`;
  const [hoverOpen, setHoverOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [popoverPos, setPopoverPos] = useState({ top: 0, left: 0 });
  const rowRef = useRef<HTMLDivElement>(null);
  const hoverTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const leaveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const showPopover = useCallback(() => {
    if (leaveTimerRef.current) clearTimeout(leaveTimerRef.current);
    hoverTimerRef.current = setTimeout(() => {
      if (rowRef.current) {
        const rect = rowRef.current.getBoundingClientRect();
        setPopoverPos({ top: rect.top, left: rect.right + 4 });
      }
      setHoverOpen(true);
    }, 250);
  }, []);

  const hidePopover = useCallback(() => {
    if (hoverTimerRef.current) clearTimeout(hoverTimerRef.current);
    leaveTimerRef.current = setTimeout(() => setHoverOpen(false), 150);
  }, []);

  const keepPopover = useCallback(() => {
    if (leaveTimerRef.current) clearTimeout(leaveTimerRef.current);
  }, []);

  const copyUrl = useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
    if (!service.url) return;
    void navigator.clipboard.writeText(service.url).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  }, [service.url]);

  const openUrl = useCallback((e: React.MouseEvent) => {
    e.stopPropagation();
    if (service.url) window.open(service.url, "_blank");
  }, [service.url]);

  return (
    <div ref={rowRef} onMouseEnter={showPopover} onMouseLeave={hidePopover}>
      <div
        onClick={handleSelect}
        className={cn(
          "service-row group cursor-pointer mx-1 rounded-sm",
          svcColorClass,
          isViewing
            ? "bg-surface-sunken"
            : "hover:bg-surface-sunken/50",
        )}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.target !== e.currentTarget) return;
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            handleSelect();
          }
        }}
        aria-label={`${name} — ${service.state}`}
        aria-pressed={isViewing}
      >
        <div className="flex items-center gap-2 px-2 py-1.5">
          <StatusDot state={service.state} />
          <span className={cn(
            "text-[13px] font-medium truncate min-w-0 flex-1",
            isViewing ? "text-ink" : "text-ink-secondary"
          )}>
            {name}
          </span>
          {service.state !== "ready" && (
            <span className={cn(
              "text-[11px] capitalize shrink-0",
              service.state === "starting" && "text-status-amber-text",
              service.state === "failed" && "text-status-red-text",
              service.state === "stopped" && "text-ink-tertiary",
              service.state === "degraded" && "text-status-amber-text",
            )}>{service.state}</span>
          )}
        </div>
      </div>

      {/* Hover popover — portaled to body to avoid overflow clipping */}
      {hoverOpen && service.url && createPortal(
        <div
          className="fixed z-50 svc-popover-enter"
          style={{ top: popoverPos.top, left: popoverPos.left }}
          onMouseEnter={keepPopover}
          onMouseLeave={hidePopover}
        >
          <div className={cn(
            "bg-surface-overlay border border-line shadow-lg rounded-md p-3 w-56",
            svcColorClass,
          )}>
            <div className="flex items-center gap-2 mb-2">
              <StatusDot state={service.state} />
              <span className="text-sm font-semibold text-ink">{name}</span>
              <span className={cn(
                "text-[11px] capitalize ml-auto",
                service.state === "ready" && "text-status-green-text",
                service.state === "starting" && "text-status-amber-text",
                service.state === "failed" && "text-status-red-text",
                service.state === "stopped" && "text-ink-tertiary",
              )}>{service.state}</span>
            </div>
            <div className="text-[11px] text-ink-tertiary font-mono truncate mb-3" title={service.url}>
              {service.url}
            </div>
            <div className="flex items-center gap-1.5">
              <button
                onClick={copyUrl}
                className="flex items-center gap-1.5 px-2.5 h-7 text-[11px] font-medium bg-surface-sunken hover:bg-surface-base border border-line rounded-md transition-colors"
              >
                {copied ? <><Check className="w-3 h-3 text-status-green-text" />Copied</> : <><Copy className="w-3 h-3" />Copy URL</>}
              </button>
              <button
                onClick={openUrl}
                className="flex items-center gap-1.5 px-2.5 h-7 text-[11px] font-medium bg-surface-sunken hover:bg-surface-base border border-line rounded-md transition-colors"
              >
                <ExternalLink className="w-3 h-3" />Open
              </button>
            </div>
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
}
