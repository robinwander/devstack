import { useState, useCallback } from "react";
import {
  ExternalLink,
  Copy,
  Check,
  RotateCcw,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { toast } from "sonner";
import { StatusIcon } from "./status-dot";
import type { ServiceStatus } from "@/lib/api";

export function ServiceRow({
  name,
  service,
  isViewing,
  onSelect,
  onRestart,
  isRestarting,
}: {
  name: string;
  service: ServiceStatus;
  isViewing: boolean;
  onSelect: ((name: string) => void) | (() => void);
  onRestart: ((name: string) => void) | (() => void);
  isRestarting: boolean;
}) {
  const [copied, setCopied] = useState(false);

  const copyUrl = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (service.url) {
      try {
        await navigator.clipboard.writeText(service.url);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
        toast.success("URL copied");
      } catch {
        toast.error("Failed to copy URL");
      }
    }
  }, [service.url]);

  const handleSelect = useCallback(() => {
    (onSelect as (name: string) => void)(name);
  }, [onSelect, name]);

  const handleRestart = useCallback(() => {
    (onRestart as (name: string) => void)(name);
  }, [onRestart, name]);

  return (
    <div
      onClick={handleSelect}
      className={cn(
        "service-row group cursor-pointer mx-1.5 mb-0.5 border-l-2",
        isViewing
          ? "bg-secondary/70 border-l-primary/70"
          : "hover:bg-secondary/30 border-l-transparent",
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
      <div className="flex items-center gap-2.5 px-3 py-3 md:py-2.5">
        {/* Status indicator */}
        <StatusIcon state={service.state} />

        {/* Name */}
        <span className={cn(
          "text-sm font-medium truncate min-w-0 flex-1",
          isViewing ? "text-foreground" : "text-foreground/70"
        )}>
          {name}
        </span>

        {/* State label (only when not ready) — no truncation, uses flex-shrink-0 */}
        {service.state !== "ready" && (
          <span className={cn(
            "text-xs capitalize shrink-0",
            service.state === "starting" && "text-amber-500",
            service.state === "failed" && "text-red-400",
            service.state === "stopped" && "text-muted-foreground/40",
            service.state === "degraded" && "text-orange-500",
          )}>{service.state}</span>
        )}

        {/* Actions — always visible at 60% opacity, 100% on hover/viewing, keyboard-focusable */}
        <div className={cn(
          "flex items-center gap-0.5 shrink-0 ml-auto transition-opacity duration-150",
          isViewing ? "opacity-100" : "opacity-60 group-hover:opacity-100"
        )}>
          {service.url && (
            <button
              onClick={copyUrl}
              className="w-10 h-10 md:w-8 md:h-8 flex items-center justify-center text-muted-foreground/50 hover:text-foreground transition-colors"
              aria-label={`Copy ${name} URL`}
              tabIndex={0}
            >
              {copied ? <Check className="w-4 h-4 md:w-3.5 md:h-3.5 text-emerald-500" /> : <Copy className="w-4 h-4 md:w-3.5 md:h-3.5" />}
            </button>
          )}
          {service.url && (
            <a
              href={service.url}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              className="w-10 h-10 md:w-8 md:h-8 flex items-center justify-center text-muted-foreground/50 hover:text-foreground transition-colors"
              aria-label={`Open ${name}`}
              tabIndex={0}
            >
              <ExternalLink className="w-4 h-4 md:w-3.5 md:h-3.5" />
            </a>
          )}
          {service.state !== "stopped" && (
            <button
              onClick={(e) => { e.stopPropagation(); handleRestart(); }}
              disabled={isRestarting}
              className="w-10 h-10 md:w-8 md:h-8 flex items-center justify-center text-muted-foreground/50 hover:text-foreground transition-colors disabled:opacity-30"
              aria-label={`Restart ${name}`}
              tabIndex={0}
            >
              <RotateCcw className={cn("w-4 h-4 md:w-3.5 md:h-3.5", isRestarting && "animate-spin")} />
            </button>
          )}
        </div>
      </div>

      {/* URL + failure info */}
      {(service.url || service.last_failure) && (
        <div className="px-3 pb-2 -mt-0.5">
          {service.url && (
            <div className="text-[11px] font-mono text-muted-foreground/45 truncate pl-[30px]">
              {service.url}
            </div>
          )}
          {service.last_failure && (
            <div className="text-[11px] text-red-400/50 truncate pl-[30px] mt-0.5" title={service.last_failure}>
              {service.last_failure}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
