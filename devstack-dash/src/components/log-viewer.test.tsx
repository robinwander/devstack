// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LogViewer } from "./log-viewer";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

function renderViewer() {
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        retry: false,
        refetchInterval: false,
      },
    },
  });

  return render(
    <QueryClientProvider client={client}>
      <LogViewer
        runId="run-1"
        projectDir="/tmp/project"
        services={["api", "worker"]}
        selectedService={null}
        onSelectService={vi.fn()}
      />
    </QueryClientProvider>,
  );
}

const logSearchResponse = {
  entries: [],
  truncated: false,
  total: 0,
  error_count: 0,
  warn_count: 0,
  matched_total: 0,
};

const facetsResponse = {
  total: 12,
  filters: [
    {
      field: "service",
      kind: "select",
      values: [
        { value: "api", count: 7 },
        { value: "worker", count: 5 },
      ],
    },
    {
      field: "level",
      kind: "toggle",
      values: [
        { value: "warn", count: 3 },
        { value: "error", count: 2 },
      ],
    },
    {
      field: "stream",
      kind: "toggle",
      values: [
        { value: "stdout", count: 10 },
        { value: "stderr", count: 2 },
      ],
    },
    {
      field: "region",
      kind: "toggle",
      values: [{ value: "debug", count: 1 }],
    },
  ],
};

describe("LogViewer facets + URL params", () => {
  beforeEach(() => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input) => {
      const url = typeof input === "string" ? input : input instanceof Request ? input.url : String(input);
      if (url.includes("/api/v1/runs/run-1/logs/facets")) {
        return jsonResponse(facetsResponse);
      }
      if (url.includes("/api/v1/runs/run-1/logs")) {
        return jsonResponse(logSearchResponse);
      }
      if (url.includes("/api/v1/agent/sessions/latest")) {
        return jsonResponse({ session: null });
      }
      throw new Error(`Unhandled fetch URL: ${url}`);
    });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    window.history.replaceState({}, "", "/");
  });

  it("renders filters from API metadata and styles unknown values neutrally", async () => {
    renderViewer();

    expect(await screen.findByText("region")).toBeTruthy();

    const debugButton = screen.getByRole("button", { name: "debug" });
    expect(debugButton.className).not.toContain("text-red");
    expect(debugButton.className).not.toContain("text-amber");
  });

  it("initializes filter state from URL params", async () => {
    window.history.replaceState(
      {},
      "",
      "/?search=panic&level=error&stream=stderr&since=15m&last=100",
    );

    renderViewer();

    await screen.findByText("level");

    const search = screen.getByLabelText("Search log lines") as HTMLInputElement;
    expect(search.value).toBe("panic");
    expect(screen.getByRole("button", { name: "error" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "stderr" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("updates URL when filters change and removes params when cleared", async () => {
    renderViewer();
    await screen.findByText("level");

    const search = screen.getByLabelText("Search log lines") as HTMLInputElement;
    fireEvent.change(search, { target: { value: "timeout" } });

    fireEvent.click(screen.getByRole("button", { name: "error" }));

    await waitFor(() => {
      expect(window.location.search).toContain("search=timeout");
      expect(window.location.search).toContain("level=error");
    });

    fireEvent.click(screen.getByRole("button", { name: /show all logs/i }));
    fireEvent.click(screen.getByRole("button", { name: /clear search/i }));

    await waitFor(() => {
      expect(window.location.search).not.toContain("search=");
      expect(window.location.search).not.toContain("level=");
    });
  });
});
