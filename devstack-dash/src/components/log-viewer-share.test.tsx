// @vitest-environment jsdom

import { vi } from 'vitest'

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * 24,
      })),
    getTotalSize: () => count * 24,
    scrollToIndex: vi.fn(),
    measureElement: vi.fn(),
  }),
}))

vi.mock('@/components/json-editor', () => ({
  JsonEditorView: ({ content }: { content: { json: unknown } }) => (
    <pre aria-label="Rendered JSON">{JSON.stringify(content.json, null, 2)}</pre>
  ),
}))

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { LogViewer } from './log-viewer'

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

function renderViewer(selectedService: string | null = null) {
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        refetchInterval: false,
      },
    },
  })

  return render(
    <QueryClientProvider client={client}>
      <LogViewer
        runId="run-1"
        projectDir="/tmp/project"
        services={['api', 'worker']}
        selectedService={selectedService}
        onSelectService={vi.fn()}
      />
    </QueryClientProvider>,
  )
}

const logSearchResponse = {
  entries: [],
  truncated: false,
  total: 0,
  filters: [],
}

const detailLogSearchResponse = {
  entries: [
    {
      ts: '2025-01-01T00:00:00.000Z',
      service: 'api',
      stream: 'stderr',
      level: 'error',
      message: 'panic mode',
      raw: 'panic mode',
      attributes: {
        event: 'extract_tool_result',
        toolname: 'bash',
      },
    },
  ],
  truncated: false,
  total: 1,
  filters: [],
}

const facetsResponse = {
  total: 0,
  filters: [],
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  window.history.replaceState({}, '', '/')
})

describe('LogViewer share button', () => {
  it('is only visible when an agent session exists', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = typeof input === 'string' ? input : input instanceof Request ? input.url : String(input)
      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse({ ...logSearchResponse, filters: facetsResponse.filters })
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })

    renderViewer()
    await screen.findByRole('log', { name: 'Service logs' })

    expect(screen.queryByRole('button', { name: /share query with agent/i })).toBeNull()
  })

  it('reconstructs current filters into a logs command and shares it', async () => {
    window.history.replaceState(
      {},
      '',
      '/?search=panic+mode&level=error&stream=stderr&since=15m&last=100',
    )

    let sharePayload: { project_dir: string; command: string; message: string } | null = null

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url = typeof input === 'string' ? input : input instanceof Request ? input.url : String(input)
      const method = init?.method ?? (input instanceof Request ? input.method : 'GET')

      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse({ ...logSearchResponse, filters: facetsResponse.filters })
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({
          session: {
            agent_id: 'agent-1',
            project_dir: '/tmp/project',
            stack: null,
            command: 'claude',
            pid: 123,
            created_at: '2025-01-01T00:00:00Z',
          },
        })
      }
      if (url.includes('/api/v1/agent/share') && method === 'POST') {
        sharePayload = JSON.parse(String(init?.body))
        return jsonResponse({ agent_id: 'agent-1', queued: 1 })
      }
      throw new Error(`Unhandled fetch URL: ${url} (${method})`)
    })

    renderViewer('api')

    const shareButton = await screen.findByRole('button', { name: /share query with agent/i })
    fireEvent.click(shareButton)

    await waitFor(() => {
      expect(sharePayload).toEqual({
        project_dir: '/tmp/project',
        command:
          'devstack show --run run-1 --service api --search "panic mode level:error stream:stderr" --since 15m --last 100',
        message: 'Can you take a look at this?',
      })
    })
  })

  it('shares a specific log entry from the detail panel', async () => {
    let clipboardText = ''
    const writeText = vi.fn(async (text: string) => { clipboardText = text })
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      writable: true,
      configurable: true,
    })

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url = typeof input === 'string' ? input : input instanceof Request ? input.url : String(input)

      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse({ ...detailLogSearchResponse, filters: facetsResponse.filters })
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })

    renderViewer('api')

    fireEvent.click(await screen.findByText('panic mode'))
    fireEvent.click(await screen.findByRole('button', { name: 'Share log entry with agent' }))

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledOnce()
      expect(clipboardText).toBe('devstack show --run run-1 --service api')
    })
  })
})
