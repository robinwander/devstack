// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { PropsWithChildren } from 'react'
import { useEventStream } from './use-event-stream'
import { queryKeys } from './api'

class MockEventSource {
  static instances: MockEventSource[] = []

  url: string
  close = vi.fn()
  private listeners = new Map<string, Set<EventListenerOrEventListenerObject>>()

  constructor(url: string) {
    this.url = url
    MockEventSource.instances.push(this)
  }

  addEventListener(type: string, listener: EventListenerOrEventListenerObject) {
    const listeners = this.listeners.get(type) ?? new Set()
    listeners.add(listener)
    this.listeners.set(type, listeners)
  }

  removeEventListener(type: string, listener: EventListenerOrEventListenerObject) {
    this.listeners.get(type)?.delete(listener)
  }

  emit(type: string, payload?: unknown) {
    const event =
      type === 'open' || type === 'error'
        ? new Event(type)
        : ({ data: JSON.stringify(payload) } as MessageEvent<string>)

    for (const listener of this.listeners.get(type) ?? []) {
      if (typeof listener === 'function') {
        listener(event)
      } else {
        listener.handleEvent(event)
      }
    }
  }

  static reset() {
    MockEventSource.instances = []
  }
}

function createWrapper(client: QueryClient) {
  return function Wrapper({ children }: PropsWithChildren) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    )
  }
}

describe('useEventStream', () => {
  beforeEach(() => {
    MockEventSource.reset()
    vi.stubGlobal('EventSource', MockEventSource)
  })

  afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it('opens an SSE stream for the active run and appends log events', async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })

    const { result } = renderHook(() => useEventStream('run-1'), {
      wrapper: createWrapper(client),
    })

    const source = MockEventSource.instances[0]
    expect(source?.url).toBe('/api/v1/events?run_id=run-1')

    source.emit('open')

    await waitFor(() => {
      expect(result.current.connected).toBe(true)
    })

    source.emit('log', {
      run_id: 'run-1',
      service: 'api',
      ts: '2025-01-01T00:00:00Z',
      stream: 'stdout',
      level: 'info',
      message: 'hello',
      raw: 'hello',
      attributes: { requestid: 'req-1' },
    })

    await waitFor(() => {
      expect(result.current.logs).toHaveLength(1)
      expect(result.current.logs[0]?.message).toBe('hello')
    })
  })

  it('invalidates queries on state events and clears logs on reconnect', async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    const invalidateQueries = vi.spyOn(client, 'invalidateQueries')

    const { result } = renderHook(() => useEventStream('run-1'), {
      wrapper: createWrapper(client),
    })

    const source = MockEventSource.instances[0]
    source.emit('open')

    await waitFor(() => {
      expect(result.current.connected).toBe(true)
    })

    source.emit('run', {
      kind: 'state_changed',
      run_id: 'run-1',
      state: 'running',
    })
    source.emit('service', {
      kind: 'state_changed',
      run_id: 'run-1',
      service: 'api',
      state: 'ready',
    })
    source.emit('task', {
      kind: 'started',
      execution_id: 'task-1',
      task: 'migrate',
      run_id: 'run-1',
      state: 'running',
      started_at: '2025-01-01T00:00:00Z',
    })
    source.emit('global', {
      kind: 'state_changed',
      key: 'db',
      state: 'running',
    })
    source.emit('log', {
      run_id: 'run-1',
      service: 'api',
      ts: '2025-01-01T00:00:00Z',
      stream: 'stdout',
      level: 'info',
      message: 'before reconnect',
      raw: 'before reconnect',
    })

    await waitFor(() => {
      expect(result.current.logs).toHaveLength(1)
    })

    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: queryKeys.runs })
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: queryKeys.runStatus('run-1'),
    })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['runs', 'run-1'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: queryKeys.globals })

    source.emit('error')

    await waitFor(() => {
      expect(result.current.connected).toBe(false)
    })

    source.emit('open')

    await waitFor(() => {
      expect(result.current.connected).toBe(true)
      expect(result.current.logs).toHaveLength(0)
    })

    expect(invalidateQueries).toHaveBeenCalledWith()
  })
})
