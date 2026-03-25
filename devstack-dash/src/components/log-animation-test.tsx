import { useState, useRef, useCallback, useEffect } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { cn } from '@/lib/utils'
import { LogRow } from './log-viewer/log-row'
import { LogScrollControls } from './log-viewer/log-scroll-controls'
import type { ParsedLog } from './log-viewer/types'

const SERVICES = ['api-server', 'voice-worker', 'test-runner'] as const
const MESSAGES = [
  "ignoring text stream with topic 'lk.agent.events', no callback attached",
  "ignoring text stream with topic 'lk.transcription', no callback attached",
  'Test agent transcript text=Hello, how can I help you today?',
  'Test agent transcript final=True',
  'Request completed: GET /health -> 200',
  'Established new Cartesia TTS WebSocket connection',
  'event loop monitor active_task_count=42.15 lag_ms=0.75',
  'conversation_item_added role=assistant',
  'LiveKit analytics metrics collected',
]
const LEVELS: ParsedLog['level'][] = ['info', 'info', 'info', 'info', 'warn', 'error']

function ts(): string {
  const d = new Date()
  const p = (n: number, w = 2) => String(n).padStart(w, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`
}

function makeFakeLog(id: number): ParsedLog {
  const svc = SERVICES[id % SERVICES.length]
  const msg = MESSAGES[id % MESSAGES.length]
  const level = LEVELS[id % LEVELS.length]
  const t = ts()
  return {
    timestamp: t,
    rawTimestamp: new Date().toISOString() + '-' + id,
    content: msg,
    service: svc,
    stream: 'stdout',
    level,
    raw: `${t} ${svc} ${msg}`,
  }
}

function buildLogKey(log: ParsedLog): string {
  return [log.rawTimestamp, log.service, log.stream, log.raw].join('\u0000')
}

export function LogAnimationTest() {
  const [logs, setLogs] = useState<ParsedLog[]>(() => {
    const initial: ParsedLog[] = []
    for (let i = 0; i < 30; i++) initial.push(makeFakeLog(i))
    return initial
  })
  const [streaming, setStreaming] = useState(false)
  const [autoScroll, setAutoScroll] = useState(true)
  const [isAtLatest, setIsAtLatest] = useState(true)
  const [newLogCount, setNewLogCount] = useState(0)
  const prevLogCountRef = useRef(logs.length)
  const nextIdRef = useRef(30)
  const containerRef = useRef<HTMLDivElement>(null)

  const logKeys = logs.map(buildLogKey)
  const seenKeysRef = useRef<Set<string> | null>(null)
  const [newKeySet, setNewKeySet] = useState<Set<string>>(new Set())

  const addBatch = useCallback((count: number) => {
    setLogs((prev) => {
      const batch: ParsedLog[] = []
      for (let i = 0; i < count; i++) batch.push(makeFakeLog(nextIdRef.current++))
      return [...batch, ...prev]
    })
  }, [])

  useEffect(() => {
    if (seenKeysRef.current === null) {
      seenKeysRef.current = new Set(logKeys)
      ;(window as any).__seeded = logKeys.length
      return
    }
    const seen = seenKeysRef.current
    const fresh = new Set<string>()
    for (const key of logKeys) {
      if (!seen.has(key)) fresh.add(key)
    }
    for (const key of logKeys) seen.add(key)
    ;(window as any).__freshCount = fresh.size
    ;(window as any).__seenSize = seen.size
    ;(window as any).__logKeysLen = logKeys.length
    if (fresh.size > 0) {
      setNewKeySet(fresh)
      const timer = setTimeout(() => setNewKeySet(new Set()), 3000)
      return () => clearTimeout(timer)
    }
  }, [logKeys])

  useEffect(() => {
    if (!streaming) return
    const id = setInterval(() => addBatch(1), 250)
    return () => clearInterval(id)
  }, [streaming, addBatch])

  useEffect(() => {
    if (autoScroll || isAtLatest) {
      setNewLogCount(0)
      prevLogCountRef.current = logs.length
    } else if (logs.length > prevLogCountRef.current) {
      setNewLogCount(logs.length - prevLogCountRef.current)
    }
  }, [logs.length, autoScroll, isAtLatest])

  const virtualizer = useVirtualizer({
    count: logs.length,
    getScrollElement: () => containerRef.current,
    estimateSize: () => 24,
    overscan: 30,
    getItemKey: (index) => logKeys[index],
  })

  useEffect(() => {
    if (autoScroll && logs.length > 0) {
      virtualizer.scrollToIndex(0, { align: 'start' })
    }
  }, [logs, autoScroll, virtualizer])

  const handleScroll = useCallback(() => {
    if (!containerRef.current) return
    const { scrollTop } = containerRef.current
    const atLatest = scrollTop < 50
    setIsAtLatest(atLatest)
    if (atLatest && !autoScroll) setAutoScroll(true)
    else if (!atLatest && autoScroll) setAutoScroll(false)
  }, [autoScroll])

  const scrollToLatest = useCallback(() => {
    const el = containerRef.current
    if (el) el.scrollTo({ top: 0, behavior: 'smooth' })
    setAutoScroll(true)
    setNewLogCount(0)
    prevLogCountRef.current = logs.length
  }, [logs.length])

  const noopIdx = useCallback((_i: number) => {}, [])
  const noopIdxBool = useCallback((_i: number, _b: boolean) => {}, [])
  const noopLog = useCallback((_l: ParsedLog) => {}, [])
  const noopFilter = useCallback(
    (_f: string, _v: string, _a: 'include' | 'exclude' | 'only') => {},
    [],
  )

  return (
    <div className="h-screen flex flex-col bg-surface-base text-ink">
      <div className="shrink-0 border-b border-line bg-surface-raised px-4 py-2 flex items-center gap-3">
        <span className="text-sm font-semibold">Log Animation Test</span>
        <button
          id="add-batch"
          onClick={() => addBatch(10)}
          className="px-3 py-1.5 text-xs border border-line rounded-md hover:bg-surface-sunken transition-colors"
        >
          Add 10 logs
        </button>
        <button
          id="toggle-stream"
          onClick={() => setStreaming((s) => !s)}
          className={cn(
            'px-3 py-1.5 text-xs border rounded-md transition-colors',
            streaming
              ? 'bg-accent/10 border-accent text-accent'
              : 'border-line hover:bg-surface-sunken',
          )}
        >
          {streaming ? 'Stop streaming' : 'Start streaming'}
        </button>
        <button
          onClick={() => {
            setLogs([])
            nextIdRef.current = 0
          }}
          className="px-3 py-1.5 text-xs border border-line rounded-md hover:bg-surface-sunken transition-colors"
        >
          Clear
        </button>
        <span className="text-xs text-ink-tertiary ml-auto tabular-nums">
          {logs.length} lines
        </span>
      </div>

      <div className="flex-1 min-h-0 flex relative">
        <div
          id="log-test-container"
          ref={containerRef}
          onScroll={handleScroll}
          className="flex-1 overflow-auto font-mono text-[13px] leading-snug"
        >
          <div
            style={{
              height: virtualizer.getTotalSize(),
              width: '100%',
              position: 'relative',
            }}
          >
            {virtualizer.getVirtualItems().map((virtualRow) => {
              const i = virtualRow.index
              const log = logs[i]
              const prevService = i > 0 ? logs[i - 1].service : null
              const showLabel = log.service !== prevService
              const svcColorIndex = SERVICES.indexOf(
                log.service as (typeof SERVICES)[number],
              )

              return (
                <LogRow
                  key={virtualRow.key}
                  virtualRow={virtualRow}
                  measureElement={virtualizer.measureElement}
                  log={log}
                  index={i}
                  lineNumber={i + 1}
                  showLabel={showLabel}
                  showServiceColumn={true}
                  serviceColumnWidth={110}
                  svcColorIndex={svcColorIndex >= 0 ? svcColorIndex : 0}
                  highlighter={null}
                  isActiveMatch={false}
                  isExpanded={false}
                  isSelected={false}
                  isNew={newKeySet.has(logKeys[i])}
                  lineWrap={false}
                  canShare={false}
                  onToggleExpand={noopIdx}
                  onSelectRow={noopIdxBool}
                  onShareLog={noopLog}
                  onFilterAction={noopFilter}
                  hasBorderTop={showLabel && i > 0}
                  dynamicColumns={[]}
                />
              )
            })}
          </div>
        </div>

        <LogScrollControls
          newLogCount={newLogCount}
          isAtLatest={isAtLatest}
          hasSearch={false}
          newestFirst={true}
          onScrollToLatest={scrollToLatest}
        />
      </div>
    </div>
  )
}
