import { useState, useMemo, useCallback } from "react";
import { Copy, Check } from "lucide-react";
import { cn } from "@/lib/utils";
import { toast } from "sonner";
import { JsonEditorView } from "@/components/json-editor";
import type { ParsedLog } from "./types";

interface LogDetailProps {
  log: ParsedLog;
  svcColorClass: string;
}

export function LogDetail({ log, svcColorClass }: LogDetailProps) {
  const [copied, setCopied] = useState(false);
  const level = log.level;

  const jsonContent = useMemo(() => {
    if (!log.json) return null;
    return { json: log.json };
  }, [log.json]);

  const copyContent = useCallback(async () => {
    try {
      const text = log.json ? JSON.stringify(log.json, null, 2) : log.content;
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  }, [log]);

  const copyLabel = log.json ? "Copy JSON" : "Copy line";

  return (
    <div className={cn("mx-4 my-1.5 log-detail-panel animate-in fade-in-0 slide-in-from-top-1 duration-150", svcColorClass)}>
      {/* Service-colored left accent */}
      <div className="flex">
        <div className="svc-strip svc-strip-bg shrink-0" />
        <div className="flex-1 min-w-0">
          {/* Header */}
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-border/20">
            <div className="flex items-center gap-2.5 text-[11px] text-muted-foreground/50">
              {log.rawTimestamp && (
                <span className="font-mono tabular-nums">{log.rawTimestamp}</span>
              )}
              {log.service && (
                <span className="svc-text px-1.5 py-0.5 font-semibold text-[11px] bg-[var(--svc-color)]/10">
                  {log.service}
                </span>
              )}
              {/* Stream: always spelled out, never abbreviated (14.3) */}
              {log.stream && (
                <span className="text-muted-foreground/40 font-mono">{log.stream}</span>
              )}
              <span
                className={cn(
                  "uppercase text-[10px] font-bold tracking-wider",
                  level === "error" && "text-red-400/80",
                  level === "warn" && "text-amber-400/80",
                  level === "info" && "text-muted-foreground/35",
                )}
              >
                {level}
              </span>
            </div>
            {/* Copy button with specific label (14.8) */}
            <button
              onClick={(e) => {
                e.stopPropagation();
                void copyContent();
              }}
              className="flex items-center gap-1 text-[11px] text-muted-foreground/35 hover:text-foreground transition-colors px-2 py-1"
              aria-label={copyLabel}
            >
              {copied ? (
                <>
                  <Check className="w-3 h-3 text-emerald-400" />
                  <span className="text-emerald-400">Copied!</span>
                </>
              ) : (
                <>
                  <Copy className="w-3 h-3" />
                  <span>{copyLabel}</span>
                </>
              )}
            </button>
          </div>
          {/* Content */}
          <div className="px-3 py-2.5" aria-label={`Log detail for line`}>
            {jsonContent ? (
              <JsonEditorView content={jsonContent} />
            ) : (
              /* break-word not break-all (14.9) */
              <pre className="text-[13px] text-foreground/60 log-content-text font-mono leading-relaxed">
                {log.content}
              </pre>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
