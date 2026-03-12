// @vitest-environment jsdom

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
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
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

    // Dynamic fields should appear with underscores replaced by spaces
    expect(await screen.findByText('method')).toBeTruthy()
    expect(screen.getByText('status code')).toBeTruthy()

    // Values should be rendered as bar-chart items (kind=select) — use title since
    // the accessible name includes both the value text and the count (e.g. "GET25")
    expect(screen.getByTitle('GET')).toBeTruthy()
    expect(screen.getByTitle('POST')).toBeTruthy()
    expect(screen.getByTitle('200')).toBeTruthy()
  })

  it('clicking a dynamic facet value adds it as a search token and shows a chip', async () => {
    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('method')

    // Click a dynamic facet value (bar-chart style, use title to find)
    fireEvent.click(screen.getByTitle('GET'))

    // Should add the token to the search input
    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    expect(search.value).toContain('method:GET')

    // Should show a removable filter chip
    const chip = screen.getByRole('button', {
      name: 'Remove method:GET filter',
    })
    expect(chip).toBeTruthy()
  })

  it('removing a dynamic filter chip removes the token from search', async () => {
    // Start with a search token already set
    window.history.replaceState({}, '', '/?search=method:GET')

    renderViewer()

    // Wait for facets to load so facetFieldSet includes "method"
    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)
    await screen.findByText('method')
    fireEvent.click(facetToggle) // close facets

    // Chip should be present
    const chip = await screen.findByRole('button', {
      name: 'Remove method:GET filter',
    })
    fireEvent.click(chip)

    // Search should be cleared
    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    expect(search.value.trim()).toBe('')
  })

  it('marks dynamic facet values as active when their token is in search', async () => {
    window.history.replaceState({}, '', '/?search=method:GET')

    renderViewer()

    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('method')

    // The GET button should be marked as active (aria-pressed=true)
    const getButton = screen.getByTitle('GET')
    expect(getButton.getAttribute('aria-pressed')).toBe('true')

    // POST should not be active
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

    // Open facets popover (defaults to closed)
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

  it('initializes filter state from URL params', async () => {
    window.history.replaceState(
      {},
      '',
      '/?search=panic&level=error&stream=stderr&since=15m&last=100',
    )

    renderViewer()

    // Open facets popover
    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('level')

    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    expect(search.value).toBe('panic')
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

    // Open facets popover
    const facetToggle = await screen.findByTitle(/facets/i)
    fireEvent.click(facetToggle)

    await screen.findByText('level')

    const search = screen.getByLabelText('Search log lines') as HTMLInputElement
    fireEvent.change(search, { target: { value: 'timeout' } })

    fireEvent.click(screen.getByRole('button', { name: 'error' }))

    await waitFor(() => {
      expect(window.location.search).toContain('search=timeout')
      expect(window.location.search).toContain('level=error')
    })

    fireEvent.click(screen.getByRole('button', { name: /show all logs/i }))
    fireEvent.click(screen.getByRole('button', { name: /clear search/i }))

    await waitFor(() => {
      expect(window.location.search).not.toContain('search=')
      expect(window.location.search).not.toContain('level=')
    })
  })
})
