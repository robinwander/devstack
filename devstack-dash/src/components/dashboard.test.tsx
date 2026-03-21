// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useState, useEffect, useRef, useMemo } from "react";
import type { RunSummary } from "@/lib/api";
import type { ActiveRun } from "./dashboard";

/**
 * Extract the core selection logic from Dashboard into a standalone hook
 * so we can test it without QueryClient / fetch mocking.
 */
function useRunSelection(runs: RunSummary[], activeRuns: ActiveRun[]) {
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [selectedService, setSelectedService] = useState<string | null>(null);
  const userSelectedRef = useRef(false);

  // Auto-select only when no valid selection exists
  useEffect(() => {
    const selectionStillValid =
      selectedRunId !== null &&
      runs.some((r) => r.run_id === selectedRunId && r.state !== "stopped");

    if (selectionStillValid) return;

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

  const selectRun = (runId: string) => {
    setSelectedRunId(runId);
    setSelectedService(null);
    userSelectedRef.current = true;
  };

  return { selectedRunId, currentRun, selectedService, selectRun };
}

// --- Test helpers ---

function makeRun(id: string, state: RunSummary["state"] = "running", createdAt = "2025-01-01T00:00:00Z"): RunSummary {
  return {
    run_id: id,
    stack: `stack-${id}`,
    project_dir: `/tmp/${id}`,
    state,
    created_at: createdAt,
    stopped_at: null,
  };
}

function toActiveRuns(runs: RunSummary[]): ActiveRun[] {
  return runs
    .filter((r) => r.state !== "stopped")
    .map((run) => ({ run, projectName: run.project_dir.split("/").pop() || "unknown" }))
    .sort((a, b) => b.run.created_at.localeCompare(a.run.created_at));
}

describe("useRunSelection", () => {
  it("auto-selects the first active run when none is selected", () => {
    const runs = [makeRun("a"), makeRun("b")];
    const active = toActiveRuns(runs);

    const { result } = renderHook(() => useRunSelection(runs, active));

    // Should auto-select the first active run (sorted by created_at desc → both same, stable order)
    expect(result.current.selectedRunId).toBe("a");
    expect(result.current.currentRun?.run_id).toBe("a");
  });

  it("preserves user selection across poll updates", () => {
    const runs1 = [makeRun("a", "running", "2025-01-01T00:00:00Z"), makeRun("b", "running", "2025-01-01T00:01:00Z")];
    const active1 = toActiveRuns(runs1);

    const { result, rerender } = renderHook(
      ({ runs, active }) => useRunSelection(runs, active),
      { initialProps: { runs: runs1, active: active1 } },
    );

    // Auto-selects "b" (newer created_at)
    expect(result.current.selectedRunId).toBe("b");

    // User explicitly selects "a"
    act(() => result.current.selectRun("a"));
    expect(result.current.selectedRunId).toBe("a");
    expect(result.current.currentRun?.run_id).toBe("a");

    // Simulate poll: same runs, new object references
    const runs2 = [makeRun("a", "running", "2025-01-01T00:00:00Z"), makeRun("b", "running", "2025-01-01T00:01:00Z")];
    const active2 = toActiveRuns(runs2);
    rerender({ runs: runs2, active: active2 });

    // Selection must stay on "a" — NOT jump to "b"
    expect(result.current.selectedRunId).toBe("a");
    expect(result.current.currentRun?.run_id).toBe("a");
  });

  it("falls back when the selected run is purged from the list", () => {
    const runs1 = [makeRun("a"), makeRun("b")];
    const active1 = toActiveRuns(runs1);

    const { result, rerender } = renderHook(
      ({ runs, active }) => useRunSelection(runs, active),
      { initialProps: { runs: runs1, active: active1 } },
    );

    // User selects "a"
    act(() => result.current.selectRun("a"));
    expect(result.current.selectedRunId).toBe("a");

    // Run "a" is purged — only "b" remains
    const runs2 = [makeRun("b")];
    const active2 = toActiveRuns(runs2);
    rerender({ runs: runs2, active: active2 });

    // Should fall back to "b"
    expect(result.current.selectedRunId).toBe("b");
    expect(result.current.currentRun?.run_id).toBe("b");
  });

  it("returns null when all runs are gone", () => {
    const runs1 = [makeRun("a")];
    const active1 = toActiveRuns(runs1);

    const { result, rerender } = renderHook(
      ({ runs, active }) => useRunSelection(runs, active),
      { initialProps: { runs: runs1, active: active1 } },
    );

    expect(result.current.selectedRunId).toBe("a");

    // All runs purged
    rerender({ runs: [], active: [] });

    expect(result.current.selectedRunId).toBeNull();
    expect(result.current.currentRun).toBeNull();
  });

  it("does not auto-switch when a new run appears", () => {
    const runs1 = [makeRun("a", "running", "2025-01-01T00:00:00Z")];
    const active1 = toActiveRuns(runs1);

    const { result, rerender } = renderHook(
      ({ runs, active }) => useRunSelection(runs, active),
      { initialProps: { runs: runs1, active: active1 } },
    );

    expect(result.current.selectedRunId).toBe("a");

    // A newer run "b" appears
    const runs2 = [makeRun("a", "running", "2025-01-01T00:00:00Z"), makeRun("b", "running", "2025-01-02T00:00:00Z")];
    const active2 = toActiveRuns(runs2);
    rerender({ runs: runs2, active: active2 });

    // Should stay on "a" — not jump to "b" even though "b" is newer
    expect(result.current.selectedRunId).toBe("a");
    expect(result.current.currentRun?.run_id).toBe("a");
  });

  it("switches to the next active run when the selected run stops", () => {
    const runs1 = [makeRun("a", "running"), makeRun("b", "running")];
    const active1 = toActiveRuns(runs1);

    const { result, rerender } = renderHook(
      ({ runs, active }) => useRunSelection(runs, active),
      { initialProps: { runs: runs1, active: active1 } },
    );

    act(() => result.current.selectRun("a"));
    expect(result.current.selectedRunId).toBe("a");

    const runs2 = [makeRun("a", "stopped"), makeRun("b", "running")];
    const active2 = toActiveRuns(runs2);
    rerender({ runs: runs2, active: active2 });

    expect(result.current.selectedRunId).toBe("b");
    expect(result.current.currentRun?.run_id).toBe("b");
  });

  it("clears selected service when switching runs", () => {
    const runs = [makeRun("a"), makeRun("b")];
    const active = toActiveRuns(runs);

    const { result } = renderHook(() => useRunSelection(runs, active));

    // Select run "b"
    act(() => result.current.selectRun("b"));
    expect(result.current.selectedService).toBeNull();
  });
});
