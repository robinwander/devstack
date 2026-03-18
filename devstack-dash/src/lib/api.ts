import { queryOptions } from '@tanstack/react-query'

const API_BASE = '/api'

export type RunLifecycle = 'starting' | 'running' | 'degraded' | 'stopped'
export type ServiceState =
  | 'starting'
  | 'ready'
  | 'degraded'
  | 'stopped'
  | 'failed'

export interface RunSummary {
  run_id: string
  stack: string
  project_dir: string
  state: RunLifecycle
  created_at: string
  stopped_at: string | null
}

export interface ServiceManifest {
  port: number | null
  url: string | null
  state: ServiceState
  watch_hash: string | null
}

export interface RunManifest {
  run_id: string
  project_dir: string
  stack: string
  manifest_path: string
  services: Record<string, ServiceManifest>
  env: Record<string, string>
  state: RunLifecycle
  created_at: string
  stopped_at: string | null
}

export interface ServiceStatus {
  desired: string
  ready: boolean
  state: ServiceState
  last_failure: string | null
  url: string | null
  systemd?: {
    active_state: string
    sub_state: string
    result: string | null
  } | null
}

export interface RunStatusResponse {
  run_id: string
  stack: string
  project_dir: string
  state: RunLifecycle
  services: Record<string, ServiceStatus>
}

export interface GlobalSummary {
  key: string
  name: string
  project_dir: string
  state: RunLifecycle
  port: number | null
  url: string | null
}

export interface SourceSummary {
  name: string
  paths: string[]
  created_at: string
}

export interface TaskExecutionSummary {
  task: string
  started_at: string
  finished_at: string
  exit_code: number
  duration_ms: number
}

export interface PingResponse {
  ok: boolean
}

export interface GcResponse {
  removed_runs: string[]
  removed_globals: string[]
}

export interface LogsResponse {
  lines: string[]
  truncated: boolean
}

export interface LogEntry {
  ts: string
  service: string
  stream: string
  level: string
  message: string
  raw: string
  attributes?: Record<string, string>
}

export interface LogViewResponse {
  entries: LogEntry[]
  truncated: boolean
  total: number
  filters: FacetFilter[]
}

export interface FacetValueCount {
  value: string
  count: number
}

export interface FacetFilter {
  field: string
  kind: string
  values: FacetValueCount[]
}

export interface ProjectSummary {
  id: string
  path: string
  name: string
  stacks: string[]
  last_used: string | null
  config_exists: boolean
}

export interface LogFilterParams {
  last?: number
  since?: string
  search?: string
  level?: string
  stream?: string
  service?: string
  include_entries?: boolean
  include_facets?: boolean
}

export interface NavigationIntent {
  run_id: string | null
  service: string | null
  search: string | null
  level: string | null
  stream: string | null
  since: string | null
  last: number | null
  created_at: string
}

export interface NavigationIntentResponse {
  intent: NavigationIntent | null
}

export interface AgentSession {
  agent_id: string
  project_dir: string
  stack: string | null
  command: string
  pid: number
  created_at: string
}

export interface LatestAgentSessionResponse {
  session: AgentSession | null
}

export interface ShareAgentMessageResponse {
  agent_id: string
  queued: number
}

function toQueryString(
  params: Record<string, string | number | boolean | null | undefined>,
): string {
  const qs = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null) continue
    qs.set(k, String(v))
  }
  const s = qs.toString()
  return s ? `?${s}` : ''
}

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

async function fetchApi<T>(
  endpoint: string,
  options?: RequestInit,
): Promise<T> {
  const res = await fetch(`${API_BASE}${endpoint}`, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...options?.headers,
    },
  })

  if (!res.ok) {
    const error = await res.json().catch(() => ({ error: res.statusText }))
    throw new ApiError(res.status, error.error || 'API request failed')
  }

  return res.json()
}

export const api = {
  ping: () => fetchApi<PingResponse>('/v1/ping'),

  listRuns: () =>
    fetchApi<{ runs: RunSummary[] }>('/v1/runs').then((r) => r.runs),

  listSources: () =>
    fetchApi<{ sources: SourceSummary[] }>('/v1/sources').then(
      (r) => r.sources,
    ),

  getRunStatus: (runId: string) =>
    fetchApi<RunStatusResponse>(`/v1/runs/${runId}/status`),

  listRunTasks: (runId: string) =>
    fetchApi<{ tasks: TaskExecutionSummary[] }>(`/v1/runs/${runId}/tasks`).then(
      (r) => r.tasks,
    ),

  runLogView: (runId: string, params: LogFilterParams) =>
    fetchApi<LogViewResponse>(
      `/v1/runs/${runId}/logs${toQueryString({
        last: params.last,
        since: params.since,
        search: params.search,
        level: params.level,
        stream: params.stream,
        service: params.service,
        include_entries: params.include_entries,
        include_facets: params.include_facets,
      })}`,
    ),

  sourceLogView: (name: string, params: LogFilterParams) =>
    fetchApi<LogViewResponse>(
      `/v1/sources/${name}/logs${toQueryString({
        last: params.last,
        since: params.since,
        search: params.search,
        level: params.level,
        stream: params.stream,
        service: params.service,
        include_entries: params.include_entries,
        include_facets: params.include_facets,
      })}`,
    ),

  listGlobals: () =>
    fetchApi<{ globals: GlobalSummary[] }>('/v1/globals').then(
      (r) => r.globals,
    ),

  up: (params: {
    stack: string
    project_dir: string
    run_id?: string
    file?: string
    no_wait?: boolean
    new_run?: boolean
    force?: boolean
  }) =>
    fetchApi<RunManifest>('/v1/runs/up', {
      method: 'POST',
      body: JSON.stringify({
        no_wait: false,
        new_run: false,
        force: false,
        ...params,
      }),
    }),

  down: (runId: string, purge = false) =>
    fetchApi<RunManifest>('/v1/runs/down', {
      method: 'POST',
      body: JSON.stringify({ run_id: runId, purge }),
    }),

  kill: (runId: string) =>
    fetchApi<RunManifest>('/v1/runs/kill', {
      method: 'POST',
      body: JSON.stringify({ run_id: runId }),
    }),

  restartService: (runId: string, service: string, noWait = false) =>
    fetchApi<RunManifest>(`/v1/runs/${runId}/restart-service`, {
      method: 'POST',
      body: JSON.stringify({ service, no_wait: noWait }),
    }),

  gc: (olderThan?: string, all = false) =>
    fetchApi<GcResponse>('/v1/gc', {
      method: 'POST',
      body: JSON.stringify({ older_than: olderThan ?? null, all }),
    }),

  getLogs: (runId: string, service: string, last = 200) =>
    fetchApi<LogsResponse>(`/v1/runs/${runId}/logs/${service}?last=${last}`),

  getNavigationIntent: () =>
    fetchApi<NavigationIntentResponse>('/v1/navigation/intent'),

  clearNavigationIntent: () =>
    fetchApi<{ ok: boolean }>('/v1/navigation/intent', {
      method: 'DELETE',
    }),

  getLatestAgentSession: (projectDir: string) =>
    fetchApi<LatestAgentSessionResponse>(
      `/v1/agent/sessions/latest${toQueryString({ project_dir: projectDir })}`,
    ),

  shareToAgent: (projectDir: string, command: string, message?: string) =>
    fetchApi<ShareAgentMessageResponse>('/v1/agent/share', {
      method: 'POST',
      body: JSON.stringify({
        project_dir: projectDir,
        command,
        message: message ?? `Can you take a look at this?`,
      }),
    }),

  listProjects: () =>
    fetchApi<{ projects: ProjectSummary[] }>('/v1/projects').then(
      (r) => r.projects,
    ),

  registerProject: (path: string) =>
    fetchApi<{ project: ProjectSummary }>('/v1/projects/register', {
      method: 'POST',
      body: JSON.stringify({ path }),
    }).then((r) => r.project),

  removeProject: (projectId: string) =>
    fetchApi<{ ok: boolean }>(`/v1/projects/${projectId}`, {
      method: 'DELETE',
    }),
}

export const queryKeys = {
  ping: ['ping'] as const,
  runs: ['runs'] as const,
  sources: ['sources'] as const,
  runStatus: (runId: string) => ['runs', runId, 'status'] as const,
  runTasks: (runId: string) => ['runs', runId, 'tasks'] as const,
  serviceLogs: (runId: string, service: string) =>
    ['runs', runId, 'logs', service] as const,
  runLogView: (runId: string, params: LogFilterParams) =>
    ['runs', runId, 'log_view', params] as const,
  sourceLogView: (name: string, params: LogFilterParams) =>
    ['sources', name, 'log_view', params] as const,
  navigationIntent: ['navigation_intent'] as const,
  latestAgentSession: (projectDir: string) =>
    ['agent_session', 'latest', projectDir] as const,
  globals: ['globals'] as const,
  projects: ['projects'] as const,
}

export const queries = {
  ping: queryOptions({
    queryKey: queryKeys.ping,
    queryFn: api.ping,
    refetchInterval: 5000,
  }),

  runs: queryOptions({
    queryKey: queryKeys.runs,
    queryFn: api.listRuns,
    refetchInterval: 3000,
    refetchOnWindowFocus: true,
  }),

  sources: queryOptions({
    queryKey: queryKeys.sources,
    queryFn: api.listSources,
    refetchInterval: 10000,
    refetchOnWindowFocus: true,
  }),

  navigationIntent: queryOptions({
    queryKey: queryKeys.navigationIntent,
    queryFn: api.getNavigationIntent,
    refetchInterval: 1000,
    refetchOnWindowFocus: true,
    retry: false,
  }),

  latestAgentSession: (projectDir: string) =>
    queryOptions({
      queryKey: queryKeys.latestAgentSession(projectDir),
      queryFn: () => api.getLatestAgentSession(projectDir),
      enabled: !!projectDir,
      refetchInterval: 2000,
      refetchOnWindowFocus: true,
      retry: false,
    }),

  runStatus: (runId: string) =>
    queryOptions({
      queryKey: queryKeys.runStatus(runId),
      queryFn: () => api.getRunStatus(runId),
      refetchInterval: (query) =>
        query.state.error instanceof ApiError &&
        query.state.error.status === 404
          ? false
          : 2000,
      refetchOnWindowFocus: true,
      retry: (count, error) =>
        error instanceof ApiError && error.status === 404 ? false : count < 3,
    }),

  runTasks: (runId: string) =>
    queryOptions({
      queryKey: queryKeys.runTasks(runId),
      queryFn: () => api.listRunTasks(runId),
      enabled: !!runId,
      refetchInterval: (query) =>
        query.state.error instanceof ApiError &&
        query.state.error.status === 404
          ? false
          : 5000,
      refetchOnWindowFocus: true,
      retry: (count, error) =>
        error instanceof ApiError && error.status === 404 ? false : count < 3,
    }),

  runLogView: (runId: string, params: LogFilterParams) =>
    queryOptions({
      queryKey: queryKeys.runLogView(runId, params),
      queryFn: () => api.runLogView(runId, params),
      enabled: !!runId,
      refetchInterval: (query) =>
        query.state.error instanceof ApiError &&
        query.state.error.status === 404
          ? false
          : 1500,
      refetchOnWindowFocus: true,
      retry: (count, error) =>
        error instanceof ApiError && error.status === 404 ? false : count < 3,
    }),

  sourceLogView: (name: string, params: LogFilterParams) =>
    queryOptions({
      queryKey: queryKeys.sourceLogView(name, params),
      queryFn: () => api.sourceLogView(name, params),
      enabled: !!name,
      refetchInterval: (query) =>
        query.state.error instanceof ApiError &&
        query.state.error.status === 404
          ? false
          : 1500,
      refetchOnWindowFocus: true,
      retry: (count, error) =>
        error instanceof ApiError && error.status === 404 ? false : count < 3,
    }),

  globals: queryOptions({
    queryKey: queryKeys.globals,
    queryFn: api.listGlobals,
    refetchInterval: 5000,
    refetchOnWindowFocus: true,
  }),

  serviceLogs: (runId: string, service: string, last = 200) =>
    queryOptions({
      queryKey: queryKeys.serviceLogs(runId, service),
      queryFn: () => api.getLogs(runId, service, last),
      refetchInterval: 2000,
      refetchOnWindowFocus: true,
    }),

  projects: queryOptions({
    queryKey: queryKeys.projects,
    queryFn: api.listProjects,
    refetchInterval: 10000,
    refetchOnWindowFocus: true,
  }),
}
