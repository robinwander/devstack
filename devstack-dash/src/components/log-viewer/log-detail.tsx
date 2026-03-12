import { useState, useMemo, useCallback } from "react";
import { Copy, Check } from "lucide-react";
import { cn } from "@/lib/utils";
import { toast } from "sonner";
import { JsonEditorView } from "@/components/json-editor";
import type { Content } from "vanilla-jsoneditor";
import type { ParsedLog } from "./types";

interface LogDetailProps {
  log: ParsedLog;
  svcColorClass: string;
}

export function LogDetail({ log, svcColorClass }: LogDetailProps) {
  const [copied, setCopied] = useState(false);
  const level = log.level;

  const editorContent = useMemo<Content | null>(() => {
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
    <div className={cn("mx-3 my-1 log-detail-panel animate-in fade-in-0 slide-in-from-top-1 duration-150", svcColorClass)}>
      <div className="flex">
        <div className="w-[3px] svc-strip-bg shrink-0 rounded-l-sm" />
        <div className="flex-1 min-w-0">
          {/* Metadata header */}
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-line-subtle">
            <div className="flex items-center gap-2 text-[11px] text-ink-tertiary font-mono">
              {log.rawTimestamp && <span className="tabular-nums">{log.rawTimestamp}</span>}
              {log.service && <span className="svc-text font-semibold">{log.service}</span>}
              {log.stream && <span>{log.stream}</span>}
              <span className={cn(
                "uppercase font-bold tracking-wider",
                level === "error" && "text-status-red-text",
                level === "warn" && "text-status-amber-text",
                level === "info" && "text-ink-tertiary",
              )}>
                {level}
              </span>
            </div>
            <button
              onClick={(e) => { e.stopPropagation(); void copyContent(); }}
              className="flex items-center gap-1 text-[11px] text-ink-tertiary hover:text-ink transition-colors px-2 py-1 rounded-sm"
              aria-label={copyLabel}
            >
              {copied ? (
                <><Check className="w-3 h-3 text-status-green-text" /><span className="text-status-green-text">Copied</span></>
              ) : (
                <><Copy className="w-3 h-3" /><span>{copyLabel}</span></>
              )}
            </button>
          </div>
          {/* Content */}
          <div className="px-3 py-2" aria-label="Log detail">
            {editorContent ? (
              <JsonEditorView content={editorContent} className="log-detail-json-editor" />
            ) : (
              <pre className="text-[13px] text-ink-secondary log-content-text font-mono leading-relaxed">
                {log.content}
              </pre>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
