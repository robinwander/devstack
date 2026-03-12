// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Dashboard } from './dashboard'

vi.mock('./header', () => ({
  Header: () => <div data-testid="header" />,
}))

vi.mock('./service-panel', () => ({
  ServicePanel: () => <div data-testid="service-panel" />,
}))

vi.mock('./log-viewer', () => ({
  LogViewer: ({ selectedService }: { selectedService: string | null }) => (
    <div data-testid="log-viewer">{selectedService ?? 'all'}</div>
  ),
}))

vi.mock('./empty-dashboard', () => ({
  EmptyDashboard: () => <div data-testid="empty-dashboard" />,
}))

vi.mock('./daemon-status', () => ({
  DaemonBanner: () => <div data-testid="daemon-banner" />,
}))

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

function coreEndpointResponse(url: string, method: string): Response | null {
  if (url.includes('/api/v1/runs/') && url.includes('/status')) {
    return jsonResponse({
      run_id: 'run-1',
      stack: 'dev',
      project_dir: '/tmp/project',
      state: 'running',
      services: {
        api: {
          desired: 'running',
          ready: true,
          state: 'ready',
          last_failure: null,
          url: 'http://localhost:3000',
        },
        worker: {
          desired: 'running',
          ready: true,
          state: 'ready',
          last_failure: null,
          url: null,
        },
      },
    })
  }

  if (url.includes('/api/v1/runs/') && url.includes('/tasks')) {
    return jsonResponse({ tasks: [] })
  }

  if (url.includes('/api/v1/runs') && method === 'GET') {
    return jsonResponse({
      runs: [
        {
          run_id: 'run-1',
          stack: 'dev',
          project_dir: '/tmp/project',
          state: 'running',
          created_at: '2025-01-01T00:00:00Z',
          stopped_at: null,
        },
      ],
    })
  }

  if (url.includes('/api/v1/projects')) {
    return jsonResponse({
      projects: [
        {
          id: 'project-1',
          name: 'project',
          path: '/tmp/project',
          stacks: ['dev'],
          last_used: null,
          config_exists: true,
        },
      ],
    })
  }

  if (url.includes('/api/v1/sources')) {
    return jsonResponse({ sources: [] })
  }

  if (url.includes('/api/v1/globals')) {
    return jsonResponse({ globals: [] })
  }

  if (url.includes('/api/v1/ping')) {
    return jsonResponse({ ok: true })
  }

  if (url.includes('/api/v1/navigation/intent') && method === 'DELETE') {
    return jsonResponse({ ok: true })
  }

  return null
}

function renderDashboard() {
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
      },
    },
  })

  return render(
    <QueryClientProvider client={client}>
      <Dashboard />
    </QueryClientProvider>,
  )
}

describe('Dashboard navigation intent polling', () => {
  beforeEach(() => {
    window.history.replaceState({}, '', '/')
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    window.history.replaceState({}, '', '/')
  })

  it('polls for navigation intents', async () => {
    let navigationIntentGets = 0

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      const method =
        init?.method ?? (input instanceof Request ? input.method : 'GET')

      if (url.includes('/api/v1/navigation/intent') && method === 'GET') {
        navigationIntentGets += 1
        return jsonResponse({ intent: null })
      }

      const response = coreEndpointResponse(url, method)
      if (response) return response

      throw new Error(`Unhandled fetch URL: ${url} (${method})`)
    })

    renderDashboard()

    await waitFor(() => {
      expect(navigationIntentGets).toBeGreaterThanOrEqual(1)
    })

    await new Promise((resolve) => setTimeout(resolve, 1100))

    await waitFor(() => {
      expect(navigationIntentGets).toBeGreaterThanOrEqual(2)
    })
  })

  it('applies and clears navigation intents', async () => {
    let clearCalls = 0
    let navigationIntentGets = 0

    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      const method =
        init?.method ?? (input instanceof Request ? input.method : 'GET')

      if (url.includes('/api/v1/navigation/intent') && method === 'GET') {
        navigationIntentGets += 1
        if (navigationIntentGets === 1) {
          return jsonResponse({
            intent: {
              run_id: 'run-1',
              service: 'worker',
              search: 'panic',
              level: 'error',
              stream: 'stderr',
              since: '2025-01-01T00:00:00Z',
              last: 75,
              created_at: '2025-01-01T00:00:01Z',
            },
          })
        }
        return jsonResponse({ intent: null })
      }

      if (url.includes('/api/v1/navigation/intent') && method === 'DELETE') {
        clearCalls += 1
        return jsonResponse({ ok: true })
      }

      const response = coreEndpointResponse(url, method)
      if (response) return response

      throw new Error(`Unhandled fetch URL: ${url} (${method})`)
    })

    renderDashboard()

    expect(await screen.findByTestId('log-viewer')).toBeTruthy()

    await waitFor(() => {
      expect(clearCalls).toBe(1)
    })

    await waitFor(() => {
      expect(window.location.search).toContain('run=run-1')
      expect(window.location.search).toContain('service=worker')
      expect(window.location.search).toContain('search=panic')
      expect(window.location.search).toContain('level=error')
      expect(window.location.search).toContain('stream=stderr')
      expect(window.location.search).toContain('since=2025-01-01T00%3A00%3A00Z')
      expect(window.location.search).toContain('last=75')
    })
  })
})
