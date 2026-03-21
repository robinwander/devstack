import { useCallback, useEffect, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import {
  queryKeys,
  type DaemonGlobalEvent,
  type DaemonLogEvent,
  type DaemonRunEvent,
  type DaemonServiceEvent,
  type LogEntry,
} from '@/lib/api'

const MAX_LOGS = 5000

function appendLogEntries(entries: LogEntry[], next: LogEntry): LogEntry[] {
  if (entries.length < MAX_LOGS) return [...entries, next]
  return [...entries.slice(entries.length - MAX_LOGS + 1), next]
}

export function useEventStream(activeRunId: string | null): {
  connected: boolean
  logs: LogEntry[]
  clearLogs: () => void
} {
  const queryClient = useQueryClient()
  const [connected, setConnected] = useState(false)
  const [logs, setLogs] = useState<LogEntry[]>([])

  const clearLogs = useCallback(() => {
    setLogs([])
  }, [])

  useEffect(() => {
    clearLogs()
    setConnected(false)

    if (typeof EventSource === 'undefined') {
      return
    }

    const params = new URLSearchParams()
    if (activeRunId) params.set('run_id', activeRunId)
    const suffix = params.toString()
    const eventSource = new EventSource(
      `/api/v1/events${suffix ? `?${suffix}` : ''}`,
    )

    let hasOpened = false
    let shouldRefreshOnOpen = false

    const handleOpen = () => {
      setConnected(true)
      if (shouldRefreshOnOpen) {
        clearLogs()
        void queryClient.invalidateQueries()
        shouldRefreshOnOpen = false
      }
      hasOpened = true
    }

    const handleError = () => {
      setConnected(false)
      if (hasOpened) {
        shouldRefreshOnOpen = true
      }
    }

    const handleRun = (event: MessageEvent<string>) => {
      const payload = JSON.parse(event.data) as DaemonRunEvent
      void queryClient.invalidateQueries({ queryKey: queryKeys.runs })
      void queryClient.invalidateQueries({
        queryKey: queryKeys.runStatus(payload.run_id),
      })
    }

    const handleService = (event: MessageEvent<string>) => {
      const payload = JSON.parse(event.data) as DaemonServiceEvent
      void queryClient.invalidateQueries({
        queryKey: queryKeys.runStatus(payload.run_id),
      })
    }

    const handleGlobal = (_event: MessageEvent<string>) => {
      const _payload = JSON.parse(_event.data) as DaemonGlobalEvent
      void queryClient.invalidateQueries({ queryKey: queryKeys.globals })
    }

    const handleLog = (event: MessageEvent<string>) => {
      const payload = JSON.parse(event.data) as DaemonLogEvent
      if (payload.run_id !== activeRunId) return
      setLogs((current) => appendLogEntries(current, payload))
    }

    eventSource.addEventListener('open', handleOpen as EventListener)
    eventSource.addEventListener('error', handleError as EventListener)
    eventSource.addEventListener('run', handleRun as EventListener)
    eventSource.addEventListener('service', handleService as EventListener)
    eventSource.addEventListener('global', handleGlobal as EventListener)
    eventSource.addEventListener('log', handleLog as EventListener)

    return () => {
      eventSource.close()
      setConnected(false)
    }
  }, [activeRunId, clearLogs, queryClient])

  return {
    connected,
    logs,
    clearLogs,
  }
}
