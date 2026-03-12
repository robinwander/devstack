import { useState, useEffect, useRef, useMemo, useCallback } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { useQuery } from '@tanstack/react-query'
import { AlertTriangle } from 'lucide-react'
import { api, ApiError, queries, type RunSummary } from '@/lib/api'
import { patchUrlParams, readUrlParam } from '@/lib/url-state'
import { useIsMobile } from '@/lib/use-media-query'
import { Header } from './header'
import { ServicePanel } from './service-panel'
import { EmptyDashboard } from './empty-dashboard'
import { LogViewer } from './log-viewer'
import { DaemonBanner } from './daemon-status'
import { CommandPalette, useDashboardCommands } from './command-palette'
import { toast } from 'sonner'

export interface ActiveRun {
  run: RunSummary
  projectName: string
}

export function Dashboard() {
  const [selectedSource, setSelectedSource] = useState<string | null>(() =>
    readUrlParam('source'),
  )
  const [selectedRunId, setSelectedRunId] = useState<string | null>(() =>
    readUrlParam('source') ? null : readUrlParam('run'),
  )
  const [selectedService, setSelectedService] = useState<string | null>(() =>
    readUrlParam('service'),
  )
  const [logViewerVersion, setLogViewerVersion] = useState(0)
  const userSelectedRef = useRef(false)
  const lastAppliedIntentRef = useRef<string | null>(null)

  const { data: runs = [] } = useQuery(queries.runs)
  const { data: sources = [] } = useQuery(queries.sources)
  const { data: projects = [] } = useQuery(queries.projects)
  const { data: globals = [] } = useQuery(queries.globals)
  const pingQuery = useQuery(queries.ping)
  const navigationIntentQuery = useQuery(queries.navigationIntent)
  const navigationIntent = navigationIntentQuery.data?.intent ?? null

  const activeRuns: ActiveRun[] = useMemo(() => {
    return runs
      .filter((r) => r.state !== 'stopped')
      .map((run) => {
        const project = projects.find((p) => p.path === run.project_dir)
        const projectName =
          project?.name || run.project_dir.split('/').pop() || 'unknown'
        return { run, projectName }
      })
      .sort((a, b) => b.run.created_at.localeCompare(a.run.created_at))
  }, [runs, projects])

  const stoppedRuns = useMemo(() => {
    return runs
      .filter((r) => r.state === 'stopped')
      .sort((a, b) =>
        (b.stopped_at || b.created_at).localeCompare(
          a.stopped_at || a.created_at,
        ),
      )
      .slice(0, 10)
  }, [runs])

  const viewMode = selectedSource ? 'source' : 'run'

  // Auto-select first active run when none selected in run mode
  useEffect(() => {
    if (viewMode === 'source') return

    const selectionStillValid =
      selectedRunId !== null && runs.some((r) => r.run_id === selectedRunId)

    if (selectionStillValid) return

    const fallback = activeRuns[0]?.run
    if (fallback) {
      setSelectedRunId(fallback.run_id)
    } else {
      setSelectedRunId(null)
    }
    userSelectedRef.current = false
  }, [viewMode, selectedRunId, runs, activeRuns])

  const currentRun = useMemo(() => {
    if (selectedRunId) {
      return runs.find((r) => r.run_id === selectedRunId) ?? null
    }
    return null
  }, [selectedRunId, runs])

  useEffect(() => {
    patchUrlParams({ run: selectedRunId })
  }, [selectedRunId])

  useEffect(() => {
    patchUrlParams({ source: selectedSource })
  }, [selectedSource])

  useEffect(() => {
    patchUrlParams({ service: selectedService })
  }, [selectedService])

  // Navigation intent from CLI
  useEffect(() => {
    if (!navigationIntent) return
    if (lastAppliedIntentRef.current === navigationIntent.created_at) return

    lastAppliedIntentRef.current = navigationIntent.created_at

    const params: Record<string, string | null | undefined> = {
      run: navigationIntent.run_id,
      service: navigationIntent.service,
      search: navigationIntent.search,
      level: navigationIntent.level,
      stream: navigationIntent.stream,
      since: navigationIntent.since,
      last:
        navigationIntent.last != null
          ? String(navigationIntent.last)
          : undefined,
    }
    patchUrlParams(params)

    if (navigationIntent.run_id) {
      setSelectedRunId(navigationIntent.run_id)
      setSelectedSource(null)
      userSelectedRef.current = true
    }
    setSelectedService(navigationIntent.service ?? null)
    setLogViewerVersion((version) => version + 1)
    void api.clearNavigationIntent()
  }, [navigationIntent])

  const currentProject = useMemo(() => {
    if (!currentRun) return null
    return projects.find((p) => p.path === currentRun.project_dir) ?? null
  }, [currentRun, projects])

  const statusQuery = useQuery({
    ...queries.runStatus(currentRun?.run_id || ''),
    enabled: viewMode === 'run' && !!currentRun,
  })
  const status = statusQuery.data

  const tasksQuery = useQuery({
    ...queries.runTasks(currentRun?.run_id || ''),
    enabled: viewMode === 'run' && !!currentRun,
  })
  const tasks = tasksQuery.data ?? []

  const isRunGone =
    statusQuery.isError &&
    statusQuery.error instanceof ApiError &&
    statusQuery.error.status === 404

  useEffect(() => {
    if (!isRunGone) return
    const fallback = activeRuns.find((ar) => ar.run.run_id !== selectedRunId)
    if (fallback) {
      setSelectedRunId(fallback.run.run_id)
      setSelectedService(null)
      userSelectedRef.current = false
    }
  }, [isRunGone, activeRuns, selectedRunId])

  const selectRun = useCallback((runId: string) => {
    setSelectedRunId(runId)
    setSelectedSource(null)
    setSelectedService(null)
    userSelectedRef.current = true
  }, [])

  const selectSource = useCallback((name: string) => {
    setSelectedSource(name)
    setSelectedRunId(null)
    setSelectedService(null)
    userSelectedRef.current = true
  }, [])

  const isDaemonDown =
    !pingQuery.isLoading && (pingQuery.isError || !pingQuery.data?.ok)

  const isMobile = useIsMobile()
  const [mobilePanelOpen, setMobilePanelOpen] = useState(false)
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false)

  useEffect(() => {
    if (!isMobile) setMobilePanelOpen(false)
  }, [isMobile])

  const handleMobileServiceSelect = useCallback(
    (name: string | null) => {
      setSelectedService(name === selectedService ? null : name)
      if (isMobile) setMobilePanelOpen(false)
    },
    [isMobile, selectedService],
  )

  // Command palette
  const [paletteOpen, setPaletteOpen] = useState(false)

  const paletteActions = useDashboardCommands({
    services: status ? Object.keys(status.services) : [],
    onSelectService: setSelectedService,
    onFocusSearch: () => {
      const el = document.querySelector<HTMLInputElement>(
        '[aria-label="Search log lines"]',
      )
      el?.focus()
      el?.select()
    },
    onToggleErrors: () => {
      window.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'e', bubbles: true }),
      )
    },
    onToggleWarns: () => {
      window.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'w', bubbles: true }),
      )
    },
    onToggleFacets: () => {
      window.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'f', bubbles: true }),
      )
    },
    onScrollToBottom: () => {
      const btn = document.querySelector<HTMLButtonElement>(
        '[aria-label="Scroll to latest"], [aria-label="Auto-scroll active"]',
      )
      btn?.click()
    },
    onCopyUrl: () => {
      void navigator.clipboard.writeText(window.location.href).then(
        () => toast.success('URL copied'),
        () => toast.error('Failed to copy URL'),
      )
    },
  })

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault()
        setPaletteOpen((v) => !v)
      }
      // Sidebar collapse/expand
      const isInput =
        document.activeElement?.tagName === 'INPUT' ||
        document.activeElement?.tagName === 'TEXTAREA'
      if (
        !isInput &&
        !e.metaKey &&
        !e.ctrlKey &&
        (e.key === '[' || e.key === ']')
      ) {
        e.preventDefault()
        setSidebarCollapsed((v) => !v)
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [])

  return (
    <div className="min-h-dvh h-dvh flex flex-col bg-surface-base text-ink overflow-hidden relative">
      {/* Skip to content link — visible only on keyboard focus */}
      <a
        href="#log-viewer"
        className="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:top-2 focus:left-2 focus:px-3 focus:py-1.5 focus:text-sm focus:font-medium focus:bg-surface-raised focus:border focus:border-line focus:rounded-md focus:shadow-lg"
      >
        Skip to logs
      </a>
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        actions={paletteActions}
      />

      <Header
        currentRun={currentRun}
        currentProject={currentProject}
        activeRuns={activeRuns}
        stoppedRuns={stoppedRuns}
        projects={projects}
        sources={sources}
        selectedSource={selectedSource}
        status={status}
        onSelectRun={selectRun}
        onSelectSource={selectSource}
        isMobile={isMobile}
        mobilePanelOpen={mobilePanelOpen}
        onToggleMobilePanel={() => setMobilePanelOpen((v) => !v)}
      />

      {isDaemonDown && <DaemonBanner onRetry={() => pingQuery.refetch()} />}

      <div className="flex-1 flex min-h-0 relative">
        <AnimatePresence mode="wait">
          {currentRun && isRunGone ? (
            <motion.div
              key="run-gone"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.15 }}
              className="flex-1 flex items-center justify-center"
            >
              <div className="text-center max-w-sm">
                <div className="w-12 h-12 mx-auto mb-4 bg-status-red-tint border border-line rounded-lg flex items-center justify-center">
                  <AlertTriangle className="w-5 h-5 text-status-red-text" />
                </div>
                <h2 className="text-lg font-semibold text-ink mb-1">
                  Run not found
                </h2>
                <p className="text-sm text-ink-secondary">
                  This run has been stopped or purged.
                </p>
              </div>
            </motion.div>
          ) : currentRun && status ? (
            <motion.div
              key={currentRun.run_id}
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.15 }}
              className="flex flex-1 min-h-0 min-w-0 overflow-hidden"
            >
              {isMobile && (
                <div
                  className={`mobile-panel-backdrop ${mobilePanelOpen ? 'is-open' : ''}`}
                  onClick={() => setMobilePanelOpen(false)}
                />
              )}

              <ServicePanel
                run={currentRun}
                status={status}
                globals={globals}
                tasks={tasks}
                selectedService={selectedService}
                selectedSource={selectedSource}
                onSelectService={
                  isMobile ? handleMobileServiceSelect : setSelectedService
                }
                isMobile={isMobile}
                mobilePanelOpen={mobilePanelOpen}
                onCloseMobilePanel={() => setMobilePanelOpen(false)}
                collapsed={!isMobile && sidebarCollapsed}
                onToggleCollapse={() => setSidebarCollapsed((v) => !v)}
              />
              <div className="flex-1 min-w-0 flex flex-col">
                <LogViewer
                  key={`${currentRun.run_id}:${logViewerVersion}`}
                  runId={currentRun.run_id}
                  projectDir={currentRun.project_dir}
                  services={Object.keys(status.services)}
                  selectedService={selectedService}
                  selectedSource={selectedSource}
                  onSelectService={setSelectedService}
                  status={status}
                  isMobile={isMobile}
                />
              </div>
            </motion.div>
          ) : selectedSource ? (
            <motion.div
              key={`source:${selectedSource}`}
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.15 }}
              className="flex flex-1 min-h-0 min-w-0 overflow-hidden"
            >
              <div className="flex-1 min-w-0 flex flex-col">
                <LogViewer
                  key={`source:${selectedSource}:${logViewerVersion}`}
                  runId=""
                  projectDir=""
                  services={[]}
                  selectedService={selectedService}
                  selectedSource={selectedSource}
                  sourceName={selectedSource}
                  onSelectService={setSelectedService}
                  isMobile={isMobile}
                />
              </div>
            </motion.div>
          ) : (
            <EmptyDashboard key="empty" projects={projects} />
          )}
        </AnimatePresence>
      </div>
    </div>
  )
}
