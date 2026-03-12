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

import type { ComponentProps } from 'react'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { LogViewer } from './log-viewer'

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { 'Content-Type': 'application/json' },
  })
}

function renderViewer(options?: Partial<ComponentProps<typeof LogViewer>>) {
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
        selectedService={null}
        onSelectService={vi.fn()}
        {...options}
      />
    </QueryClientProvider>,
  )
}

const logSearchResponse = {
  entries: [],
  truncated: false,
  total: 0,
  error_count: 0,
  warn_count: 0,
  matched_total: 0,
}

const facetsResponse = {
  total: 12,
  filters: [
    {
      field: 'service',
      kind: 'select',
      values: [
        { value: 'api', count: 7 },
        { value: 'worker', count: 5 },
      ],
    },
    {
      field: 'level',
      kind: 'toggle',
      values: [
        { value: 'warn', count: 3 },
        { value: 'error', count: 2 },
      ],
    },
    {
      field: 'stream',
      kind: 'toggle',
      values: [
        { value: 'stdout', count: 10 },
        { value: 'stderr', count: 2 },
      ],
    },
    {
      field: 'region',
      kind: 'toggle',
      values: [{ value: 'debug', count: 1 }],
    },
  ],
}

const dynamicFacetsResponse = {
  total: 50,
  filters: [
    {
      field: 'service',
      kind: 'select',
      values: [
        { value: 'api', count: 30 },
        { value: 'worker', count: 20 },
      ],
    },
    {
      field: 'level',
      kind: 'toggle',
      values: [
        { value: 'info', count: 40 },
        { value: 'error', count: 10 },
      ],
    },
    {
      field: 'stream',
      kind: 'toggle',
      values: [
        { value: 'stdout', count: 45 },
        { value: 'stderr', count: 5 },
      ],
    },
    {
      field: 'method',
      kind: 'select',
      values: [
        { value: 'GET', count: 25 },
        { value: 'POST', count: 15 },
        { value: 'DELETE', count: 5 },
      ],
    },
    {
      field: 'status_code',
      kind: 'select',
      values: [
        { value: '200', count: 30 },
        { value: '404', count: 10 },
        { value: '500', count: 5 },
      ],
    },
  ],
}

describe('LogViewer dynamic field facets', () => {
  beforeEach(() => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      if (url.includes('/api/v1/runs/run-1/logs/facets')) {
        return jsonResponse(dynamicFacetsResponse)
      }
      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse(logSearchResponse)
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    window.history.replaceState({}, '', '/')
  })

  it('renders dynamic field facets in the facet panel with formatted names', async () => {
    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    expect(await screen.findByText('method')).toBeTruthy()
    expect(screen.getByText('status code')).toBeTruthy()
    expect(screen.getByTitle('GET')).toBeTruthy()
    expect(screen.getByTitle('POST')).toBeTruthy()
    expect(screen.getByTitle('200')).toBeTruthy()
  })

  it('clicking a dynamic facet value adds it as a search token', async () => {
    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('method')
    fireEvent.click(screen.getByTitle('GET'))

    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    expect(search.value).toContain('method:GET')
  })

  it('clicking an active dynamic facet value removes the token from search', async () => {
    window.history.replaceState({}, '', '/?search=method:GET')

    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)
    await screen.findByText('method')
    fireEvent.click(screen.getByTitle('GET'))

    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    expect(search.value.trim()).toBe('')
  })

  it('marks dynamic facet values as active when their token is in search', async () => {
    window.history.replaceState({}, '', '/?search=method:GET')

    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('method')

    const getButton = screen.getByTitle('GET')
    expect(getButton.getAttribute('aria-pressed')).toBe('true')

    const postButton = screen.getByTitle('POST')
    expect(postButton.getAttribute('aria-pressed')).toBe('false')
  })
})

describe('LogViewer source routing', () => {
  beforeEach(() => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      if (url.includes('/api/v1/sources/ext-logs/facets')) {
        return jsonResponse(facetsResponse)
      }
      if (url.includes('/api/v1/sources/ext-logs/logs')) {
        return jsonResponse(logSearchResponse)
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    window.history.replaceState({}, '', '/')
  })

  it('uses source log endpoints when a source is selected', async () => {
    renderViewer({
      runId: '',
      projectDir: '',
      services: [],
      selectedSource: 'ext-logs',
      sourceName: 'ext-logs',
    })

    await screen.findByRole('log', { name: 'Service logs' })

    const fetchMock = vi.mocked(globalThis.fetch)
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/v1/sources/ext-logs/facets'),
      expect.anything(),
    )
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining('/api/v1/sources/ext-logs/logs'),
      expect.anything(),
    )
  })
})

describe('LogViewer facets + URL params', () => {
  beforeEach(() => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      if (url.includes('/api/v1/runs/run-1/logs/facets')) {
        return jsonResponse(facetsResponse)
      }
      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse(logSearchResponse)
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    window.history.replaceState({}, '', '/')
  })

  it('renders filters from API metadata and styles unknown values neutrally', async () => {
    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    expect(await screen.findByText('region')).toBeTruthy()

    const debugButton = screen.getByRole('button', { name: 'debug' })
    expect(debugButton.className).not.toContain('text-red')
    expect(debugButton.className).not.toContain('text-amber')
  })

  it('shows each facet suggestion only once for an empty search token', async () => {
    renderViewer()

    const search = await screen.findByLabelText('Search log lines')
    fireEvent.focus(search)

    const suggestions = await screen.findByRole('listbox', {
      name: 'Search suggestions',
    })
    expect(within(suggestions).getAllByRole('option')).toHaveLength(4)
  })

  it('initializes filter state from URL params (migrating legacy level/stream)', async () => {
    window.history.replaceState(
      {},
      '',
      '/?search=panic&level=error&stream=stderr&since=15m&last=100',
    )

    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('level')

    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    expect(search.value).toContain('panic')
    expect(search.value).toContain('level:error')
    expect(search.value).toContain('stream:stderr')

    expect(
      screen
        .getByRole('button', { name: 'error' })
        .getAttribute('aria-pressed'),
    ).toBe('true')
    expect(
      screen
        .getByRole('button', { name: 'stderr' })
        .getAttribute('aria-pressed'),
    ).toBe('true')
  })

  it('updates URL when filters change and removes params when cleared', async () => {
    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('level')

    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    fireEvent.change(search, { target: { value: 'timeout' } })

    fireEvent.click(screen.getByRole('button', { name: 'error' }))

    await waitFor(() => {
      expect(window.location.search).toContain('search=')
      const searchParam = new URLSearchParams(window.location.search).get('search')
      expect(searchParam).toContain('level:error')
    })

    fireEvent.click(screen.getByRole('button', { name: /clear search/i }))

    await waitFor(() => {
      expect(window.location.search).not.toContain('search=')
    })
  })
})

describe('LogViewer view toggles', () => {
  const logEntries = {
    entries: [
      {
        ts: '2025-01-01T00:00:00.000Z',
        service: 'api',
        stream: 'stdout',
        level: 'info',
        message: 'older message',
        raw: 'older message',
      },
      {
        ts: '2025-01-01T00:00:01.000Z',
        service: 'api',
        stream: 'stdout',
        level: 'info',
        message: 'newest message',
        raw: 'newest message',
      },
    ],
    truncated: false,
    total: 2,
    error_count: 0,
    warn_count: 0,
    matched_total: 0,
  }

  beforeEach(() => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      if (url.includes('/api/v1/runs/run-1/logs/facets')) {
        return jsonResponse(facetsResponse)
      }
      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse(logEntries)
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    window.localStorage.clear()
    window.history.replaceState({}, '', '/')
  })

  it('defaults to newest-first sorting and persists the toggle to localStorage', async () => {
    renderViewer()

    const newest = await screen.findByText('newest message')
    const older = await screen.findByText('older message')
    expect(newest.compareDocumentPosition(older) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(window.localStorage.getItem('devstack:log-viewer:sort-direction')).toBe(
      'desc',
    )

    fireEvent.click(screen.getByRole('button', { name: 'Newest first' }))

    await waitFor(() => {
      expect(
        window.localStorage.getItem('devstack:log-viewer:sort-direction'),
      ).toBe('asc')
    })

    cleanup()
    renderViewer()

    const oldestFirstButton = await screen.findByRole('button', {
      name: 'Oldest first',
    })
    expect(oldestFirstButton.getAttribute('aria-pressed')).toBe('false')

    const olderAfterRerender = await screen.findByText('older message')
    const newestAfterRerender = await screen.findByText('newest message')
    expect(
      olderAfterRerender.compareDocumentPosition(newestAfterRerender) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).not.toBe(0)
  })

  it('toggles line wrapping and persists the preference', async () => {
    renderViewer()

    const message = await screen.findByText('newest message')
    expect(message.className).toContain('whitespace-nowrap')
    expect(window.localStorage.getItem('devstack:log-viewer:line-wrap')).toBe(
      'false',
    )

    fireEvent.click(screen.getByRole('button', { name: 'Line wrap off' }))

    await waitFor(() => {
      expect(window.localStorage.getItem('devstack:log-viewer:line-wrap')).toBe(
        'true',
      )
      expect(screen.getByText('newest message').className).toContain(
        'whitespace-pre-wrap',
      )
    })
  })
})

describe('LogViewer detail actions, selection, and custom time range', () => {
  const detailEntries = {
    entries: [
      {
        ts: '2025-01-01T10:00:00.000Z',
        service: 'api',
        stream: 'stdout',
        level: 'info',
        message: 'first message',
        raw: 'first message',
        attributes: {
          event: 'extract_tool_result',
          toolname: 'bash',
        },
      },
      {
        ts: '2025-01-01T11:00:00.000Z',
        service: 'api',
        stream: 'stderr',
        level: 'error',
        message: 'second message',
        raw: 'second message',
        attributes: {
          event: 'task_failed',
          toolname: 'grep',
        },
      },
      {
        ts: '2025-01-01T12:00:00.000Z',
        service: 'worker',
        stream: 'stdout',
        level: 'warn',
        message: 'third message',
        raw: 'third message',
        attributes: {
          event: 'task_retried',
          toolname: 'bash',
        },
      },
    ],
    truncated: false,
    total: 3,
    error_count: 1,
    warn_count: 1,
    matched_total: 0,
  }

  beforeEach(() => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      const url =
        typeof input === 'string'
          ? input
          : input instanceof Request
            ? input.url
            : String(input)
      if (url.includes('/api/v1/runs/run-1/logs/facets')) {
        return jsonResponse(dynamicFacetsResponse)
      }
      if (url.includes('/api/v1/runs/run-1/logs')) {
        return jsonResponse(detailEntries)
      }
      if (url.includes('/api/v1/agent/sessions/latest')) {
        return jsonResponse({ session: null })
      }
      throw new Error(`Unhandled fetch URL: ${url}`)
    })
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    window.history.replaceState({}, '', '/')
  })

  it('applies filter, exclude, and only actions from the detail panel into the search bar', async () => {
    renderViewer()

    fireEvent.click(await screen.findByText('third message'))

    fireEvent.click(await screen.findByRole('button', {
      name: 'Filter to event: task_retried',
    }))
    expect((screen.getByLabelText('Search log lines') as HTMLInputElement).value).toBe(
      'event:task_retried',
    )

    fireEvent.click(screen.getByRole('button', {
      name: 'Exclude toolname: bash',
    }))
    expect((screen.getByLabelText('Search log lines') as HTMLInputElement).value).toBe(
      'event:task_retried -toolname:bash',
    )

    fireEvent.click(screen.getByRole('button', {
      name: 'Only stream: stdout',
    }))
    expect((screen.getByLabelText('Search log lines') as HTMLInputElement).value).toBe(
      'stream:stdout',
    )
  })

  it('supports selecting rows, shift-selecting ranges, and clearing the selection bar', async () => {
    renderViewer()

    fireEvent.click(await screen.findByRole('button', { name: 'Select row 1' }))
    fireEvent.click(screen.getByRole('button', { name: 'Select row 3' }), {
      shiftKey: true,
    })

    expect(await screen.findByText('3 selected')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: 'Copy selected rows' }))
    expect(navigator.clipboard.writeText).toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Clear selected rows' }))
    await waitFor(() => {
      expect(screen.queryByText('3 selected')).toBeNull()
    })
  })

  it('stores custom time ranges in the URL and applies the upper bound client-side', async () => {
    renderViewer()

    fireEvent.click(await screen.findByRole('radio', { name: 'Custom time range' }))

    fireEvent.change(screen.getByLabelText('Custom time from'), {
      target: { value: '2025-01-01T10:30:00Z' },
    })
    fireEvent.change(screen.getByLabelText('Custom time to'), {
      target: { value: '2025-01-01T11:30:00Z' },
    })
    fireEvent.click(screen.getByRole('button', { name: 'Apply' }))

    await waitFor(() => {
      const params = new URLSearchParams(window.location.search)
      expect(params.get('since')).toBe('2025-01-01T10:30:00Z')
      expect(params.get('until')).toBe('2025-01-01T11:30:00Z')
    })

    expect(screen.queryByText('third message')).toBeNull()
    expect(await screen.findByText('second message')).toBeTruthy()
  })
})
