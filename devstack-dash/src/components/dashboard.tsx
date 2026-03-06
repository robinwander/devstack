import { useState, useEffect, useRef, useMemo, useCallback } from "react";
import { motion, AnimatePresence } from "framer-motion";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle } from "lucide-react";
import { api, ApiError, queries, type RunSummary } from "@/lib/api";
import { patchUrlParams, readUrlParam } from "@/lib/url-state";
import { useIsMobile } from "@/lib/use-media-query";
import { Header } from "./header";
import { ServicePanel } from "./service-panel";
import { EmptyDashboard } from "./empty-dashboard";
import { LogViewer } from "./log-viewer";
import { DaemonBanner } from "./daemon-status";
import { CommandPalette, useDashboardCommands } from "./command-palette";
import { toast } from "sonner";

export interface ActiveRun {
  run: RunSummary;
  projectName: string;
}

export function Dashboard() {
  const [selectedRunId, setSelectedRunId] = useState<string | null>(() => readUrlParam("run"));
  const [selectedService, setSelectedService] = useState<string | null>(() => readUrlParam("service"));
  const [logViewerVersion, setLogViewerVersion] = useState(0);
  // Track whether the user explicitly picked a run (vs auto-selection)
  const userSelectedRef = useRef(false);
  const lastAppliedIntentRef = useRef<string | null>(null);

  const { data: runs = [] } = useQuery(queries.runs);
  const { data: projects = [] } = useQuery(queries.projects);
  const { data: globals = [] } = useQuery(queries.globals);
  const pingQuery = useQuery(queries.ping);
  const navigationIntentQuery = useQuery(queries.navigationIntent);
  const navigationIntent = navigationIntentQuery.data?.intent ?? null;

  const activeRuns: ActiveRun[] = useMemo(() => {
    return runs
      .filter((r) => r.state !== "stopped")
      .map((run) => {
        const project = projects.find((p) => p.path === run.project_dir);
        const projectName = project?.name || run.project_dir.split("/").pop() || "unknown";
        return { run, projectName };
      })
      .sort((a, b) => b.run.created_at.localeCompare(a.run.created_at));
  }, [runs, projects]);

  const stoppedRuns = useMemo(() => {
    return runs
      .filter((r) => r.state === "stopped")
      .sort((a, b) => (b.stopped_at || b.created_at).localeCompare(a.stopped_at || a.created_at))
      .slice(0, 10);
  }, [runs]);

  // --- Sticky run selection (Bug 1) ---
  // Auto-select only when no valid selection exists.
  useEffect(() => {
    const selectionStillValid =
      selectedRunId !== null && runs.some((r) => r.run_id === selectedRunId);

    if (selectionStillValid) return;

    // Selected run was purged or nothing selected yet — pick first active run.
    const fallback = activeRuns[0]?.run;
    if (fallback) {
      setSelectedRunId(fallback.run_id);
    } else {
      setSelectedRunId(null);
    }
    userSelectedRef.current = false;
  }, [selectedRunId, runs, activeRuns]);

  const currentRun = useMemo(() => {
    if (selectedRunId) {
      return runs.find((r) => r.run_id === selectedRunId) ?? null;
    }
    return null;
  }, [selectedRunId, runs]);

  useEffect(() => {
    patchUrlParams({ run: selectedRunId });
  }, [selectedRunId]);

  useEffect(() => {
    patchUrlParams({ service: selectedService });
  }, [selectedService]);

  useEffect(() => {
    if (!navigationIntent) return;
    if (lastAppliedIntentRef.current === navigationIntent.created_at) return;

    lastAppliedIntentRef.current = navigationIntent.created_at;

    // Apply URL params first (synchronous DOM update) so that when the
    // LogViewer remounts it reads the correct initial state from the URL.
    const params: Record<string, string | null | undefined> = {
      run: navigationIntent.run_id,
      service: navigationIntent.service,
      search: navigationIntent.search,
      level: navigationIntent.level,
      stream: navigationIntent.stream,
      since: navigationIntent.since,
      last: navigationIntent.last != null ? String(navigationIntent.last) : undefined,
    };
    patchUrlParams(params);

    if (navigationIntent.run_id) {
      setSelectedRunId(navigationIntent.run_id);
      userSelectedRef.current = true;
    }
    setSelectedService(navigationIntent.service ?? null);
    setLogViewerVersion((version) => version + 1);
    void api.clearNavigationIntent();
  }, [navigationIntent]);

  const currentProject = useMemo(() => {
    if (!currentRun) return null;
    return projects.find((p) => p.path === currentRun.project_dir) ?? null;
  }, [currentRun, projects]);

  const statusQuery = useQuery({
    ...queries.runStatus(currentRun?.run_id || ""),
    enabled: !!currentRun,
  });
  const status = statusQuery.data;

  // Detect 404 on the selected run's status (Bug 2)
  const isRunGone =
    statusQuery.isError &&
    statusQuery.error instanceof ApiError &&
    statusQuery.error.status === 404;

  // When status returns 404 and there are other active runs, auto-switch (Bug 2)
  useEffect(() => {
    if (!isRunGone) return;
    const fallback = activeRuns.find((ar) => ar.run.run_id !== selectedRunId);
    if (fallback) {
      setSelectedRunId(fallback.run.run_id);
      setSelectedService(null);
      userSelectedRef.current = false;
    }
  }, [isRunGone, activeRuns, selectedRunId]);

  // User explicitly selects a run — mark as sticky
  const selectRun = useCallback((runId: string) => {
    setSelectedRunId(runId);
    setSelectedService(null);
    userSelectedRef.current = true;
  }, []);

  const isDaemonDown = !pingQuery.isLoading && (pingQuery.isError || !pingQuery.data?.ok);

  // Mobile responsiveness
  const isMobile = useIsMobile();
  const [mobilePanelOpen, setMobilePanelOpen] = useState(false);

  // Close mobile panel when switching to desktop
  useEffect(() => {
    if (!isMobile) setMobilePanelOpen(false);
  }, [isMobile]);

  // Close mobile panel when a service is selected on mobile
  const handleMobileServiceSelect = useCallback((name: string | null) => {
    setSelectedService(name === selectedService ? null : name);
    if (isMobile) setMobilePanelOpen(false);
  }, [isMobile, selectedService]);

  // Command palette (⌘K)
  const [paletteOpen, setPaletteOpen] = useState(false);

  const paletteActions = useDashboardCommands({
    services: status ? Object.keys(status.services) : [],
    onSelectService: setSelectedService,
    onFocusSearch: () => {
      // Focus search via DOM query — the search input has aria-label="Search log lines"
      const el = document.querySelector<HTMLInputElement>('[aria-label="Search log lines"]');
      el?.focus();
      el?.select();
    },
    onToggleErrors: () => {
      // Dispatch a synthetic keydown event for 'e' to toggle error filter
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "e", bubbles: true }));
    },
    onToggleWarns: () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "w", bubbles: true }));
    },
    onToggleFacets: () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "f", bubbles: true }));
    },
    onScrollToBottom: () => {
      // The scroll-to-bottom button in LogViewer — click it
      const btn = document.querySelector<HTMLButtonElement>('[aria-label="Scroll to latest"], [aria-label="Auto-scroll active"]');
      btn?.click();
    },
    onCopyUrl: () => {
      void navigator.clipboard.writeText(window.location.href).then(
        () => toast.success("URL copied"),
        () => toast.error("Failed to copy URL"),
      );
    },
  });

  // Global ⌘K handler
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "k") {
        e.preventDefault();
        setPaletteOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  return (
    <div className="min-h-[100dvh] h-[100dvh] flex flex-col bg-background text-foreground overflow-hidden noise-bg relative">
      {/* Command Palette (⌘K) */}
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
        status={status}
        onSelectRun={selectRun}
        isMobile={isMobile}
        mobilePanelOpen={mobilePanelOpen}
        onToggleMobilePanel={() => setMobilePanelOpen((v) => !v)}
      />

      {/* Daemon unreachable banner (7.2) */}
      {isDaemonDown && <DaemonBanner onRetry={() => pingQuery.refetch()} />}

      {/* Main content */}
      <div className="flex-1 flex min-h-0 relative z-10">
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
                <div className="w-12 h-12 mx-auto mb-4 bg-red-500/5 border border-red-500/10 flex items-center justify-center">
                  <AlertTriangle className="w-5 h-5 text-red-400/40" />
                </div>
                <h2 className="text-lg font-semibold text-foreground/80 mb-1">Run not found</h2>
                <p className="text-sm text-muted-foreground/60">
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
              {/* Mobile overlay backdrop */}
              {isMobile && (
                <div
                  className={`mobile-panel-backdrop ${mobilePanelOpen ? "is-open" : ""}`}
                  onClick={() => setMobilePanelOpen(false)}
                />
              )}

              <ServicePanel
                run={currentRun}
                status={status}
                globals={globals}
                selectedService={selectedService}
                onSelectService={isMobile ? handleMobileServiceSelect : setSelectedService}
                isMobile={isMobile}
                mobilePanelOpen={mobilePanelOpen}
                onCloseMobilePanel={() => setMobilePanelOpen(false)}
              />
              <div className="flex-1 min-w-0 flex flex-col">
                <LogViewer
                  key={`${currentRun.run_id}:${logViewerVersion}`}
                  runId={currentRun.run_id}
                  projectDir={currentRun.project_dir}
                  services={Object.keys(status.services)}
                  selectedService={selectedService}
                  onSelectService={setSelectedService}
                  status={status}
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
  );
}
