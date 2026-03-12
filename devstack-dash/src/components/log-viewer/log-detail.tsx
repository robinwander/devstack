import { useState, useMemo, useCallback } from 'react'
import { Copy, Check, Share2, Equal } from 'lucide-react'
import { cn } from '@/lib/utils'
import { toast } from 'sonner'
import { JsonEditorView } from '@/components/json-editor'
import type { Content } from 'vanilla-jsoneditor'
import type { ParsedLog } from './types'

export type DetailFilterAction = 'include' | 'exclude' | 'only'

interface LogDetailProps {
  log: ParsedLog
  svcColorClass: string
  canShare: boolean
  onShare: (log: ParsedLog) => void
  onFilterAction: (
    field: string,
    value: string,
    action: DetailFilterAction,
  ) => void
}

interface ActionableField {
  field: string
  value: string
}

export function LogDetail({
  log,
  svcColorClass,
  canShare,
  onShare,
  onFilterAction,
}: LogDetailProps) {
  const [copied, setCopied] = useState(false)
  const level = log.level

  const editorContent = useMemo<Content | null>(() => {
    if (!log.json) return null
    return { json: log.json }
  }, [log.json])

  const actionableFields = useMemo<ActionableField[]>(() => {
    const fields: ActionableField[] = []
    if (log.service) fields.push({ field: 'service', value: log.service })
    if (log.stream) fields.push({ field: 'stream', value: log.stream })
    if (log.level) fields.push({ field: 'level', value: log.level })
    if (log.attributes) {
      for (const [field, value] of Object.entries(log.attributes)) {
        fields.push({ field, value })
      }
    }
    return fields
  }, [log.attributes, log.level, log.service, log.stream])

  const copyContent = useCallback(async () => {
    try {
      const text = log.json ? JSON.stringify(log.json, null, 2) : log.content
      await navigator.clipboard.writeText(text)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      toast.error('Failed to copy to clipboard')
    }
  }, [log])

  const copyLabel = log.json ? 'Copy JSON' : 'Copy line'

  return (
    <div
      className={cn(
        'mx-3 my-1 log-detail-panel animate-in fade-in-0 slide-in-from-top-1 duration-150',
        svcColorClass,
      )}
    >
      <div className="flex">
        <div className="w-[3px] svc-strip-bg shrink-0 rounded-l-sm" />
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-line-subtle gap-3">
            <div className="flex items-center gap-2 text-[11px] text-ink-tertiary font-mono min-w-0 flex-wrap">
              {log.rawTimestamp && (
                <span className="tabular-nums">{log.rawTimestamp}</span>
              )}
              {log.service && (
                <span className="svc-text font-semibold">{log.service}</span>
              )}
              {log.stream && <span>{log.stream}</span>}
              <span
                className={cn(
                  'uppercase font-bold tracking-wider',
                  level === 'error' && 'text-status-red-text',
                  level === 'warn' && 'text-status-amber-text',
                  level === 'info' && 'text-ink-tertiary',
                )}
              >
                {level}
              </span>
            </div>
            <div className="flex items-center gap-1.5 shrink-0">
              {canShare && (
                <button
                  onClick={(e) => {
                    e.stopPropagation()
                    onShare(log)
                  }}
                  className="flex items-center gap-1 text-[11px] text-ink-tertiary hover:text-ink transition-colors px-2 py-1 rounded-sm"
                  aria-label="Share log entry with agent"
                >
                  <Share2 className="w-3 h-3" />
                  <span>Share with Agent</span>
                </button>
              )}
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  void copyContent()
                }}
                className="flex items-center gap-1 text-[11px] text-ink-tertiary hover:text-ink transition-colors px-2 py-1 rounded-sm"
                aria-label={copyLabel}
              >
                {copied ? (
                  <>
                    <Check className="w-3 h-3 text-status-green-text" />
                    <span className="text-status-green-text">Copied</span>
                  </>
                ) : (
                  <>
                    <Copy className="w-3 h-3" />
                    <span>{copyLabel}</span>
                  </>
                )}
              </button>
            </div>
          </div>

          {actionableFields.length > 0 && (
            <div className="px-3 py-2 border-b border-line-subtle bg-surface-sunken/50">
              <div className="flex flex-wrap gap-2">
                {actionableFields.map(({ field, value }) => {
                  const actionLabel = `${field}: ${value}`
                  return (
                    <div
                      key={`${field}:${value}`}
                      className="inline-flex items-center gap-1 rounded-md border border-line bg-surface-base px-2 py-1 min-w-0"
                      onClick={(e) => e.stopPropagation()}
                    >
                      <span className="text-[10px] uppercase tracking-wider text-ink-tertiary font-semibold shrink-0">
                        {field}
                      </span>
                      <span
                        className="text-[11px] font-mono text-ink-secondary truncate max-w-[220px]"
                        title={value}
                      >
                        {value}
                      </span>
                      <div className="flex items-center gap-0.5 shrink-0">
                        <DetailActionButton
                          label={`Filter to ${actionLabel}`}
                          onClick={() => onFilterAction(field, value, 'include')}
                        >
                          +
                        </DetailActionButton>
                        <DetailActionButton
                          label={`Exclude ${actionLabel}`}
                          onClick={() => onFilterAction(field, value, 'exclude')}
                        >
                          −
                        </DetailActionButton>
                        <DetailActionButton
                          label={`Only ${actionLabel}`}
                          onClick={() => onFilterAction(field, value, 'only')}
                        >
                          <Equal className="w-3 h-3" />
                        </DetailActionButton>
                      </div>
                    </div>
                  )
                })}
              </div>
            </div>
          )}

          <div className="px-3 py-2" aria-label="Log detail">
            {editorContent ? (
              <JsonEditorView
                content={editorContent}
                className="log-detail-json-editor"
              />
            ) : (
              <pre className="text-[13px] text-ink-secondary log-content-text font-mono leading-relaxed">
                {log.content}
              </pre>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

function DetailActionButton({
  label,
  onClick,
  children,
}: {
  label: string
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      className="w-5 h-5 inline-flex items-center justify-center rounded-sm text-[11px] text-ink-tertiary hover:text-ink hover:bg-surface-sunken transition-colors"
      aria-label={label}
      type="button"
    >
      {children}
    </button>
  )
}
