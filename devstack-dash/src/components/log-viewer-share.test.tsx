// @vitest-environment jsdom

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LogViewer } from "./log-viewer";

function jsonResponse(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

function renderViewer(selectedService: string | null = null) {
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
        selectedService={selectedService}
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
  total: 0,
  filters: [],
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.history.replaceState({}, "", "/");
});

describe("LogViewer share button", () => {
  it("is only visible when an agent session exists", async () => {
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

    renderViewer();
    await screen.findByRole("log", { name: "Service logs" });

    expect(screen.queryByRole("button", { name: /share query with agent/i })).toBeNull();
  });

  it("reconstructs current filters into a logs command and shares it", async () => {
    window.history.replaceState(
      {},
      "",
      "/?search=panic+mode&level=error&stream=stderr&since=15m&last=100",
    );

    let sharePayload: { project_dir: string; command: string; message: string } | null = null;

    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof Request ? input.url : String(input);
      const method = init?.method ?? (input instanceof Request ? input.method : "GET");

      if (url.includes("/api/v1/runs/run-1/logs/facets")) {
        return jsonResponse(facetsResponse);
      }
      if (url.includes("/api/v1/runs/run-1/logs")) {
        return jsonResponse(logSearchResponse);
      }
      if (url.includes("/api/v1/agent/sessions/latest")) {
        return jsonResponse({
          session: {
            agent_id: "agent-1",
            project_dir: "/tmp/project",
            stack: null,
            command: "claude",
            pid: 123,
            created_at: "2025-01-01T00:00:00Z",
          },
        });
      }
      if (url.includes("/api/v1/agent/share") && method === "POST") {
        sharePayload = JSON.parse(String(init?.body));
        return jsonResponse({ agent_id: "agent-1", queued: 1 });
      }
      throw new Error(`Unhandled fetch URL: ${url} (${method})`);
    });

    renderViewer("api");

    const shareButton = await screen.findByRole("button", { name: /share query with agent/i });
    fireEvent.click(shareButton);

    await waitFor(() => {
      expect(sharePayload).toEqual({
        project_dir: "/tmp/project",
        command:
          'devstack show --run run-1 --service api --search "panic mode level:error stream:stderr" --since 15m --last 100',
        message: "Can you take a look at this?",
      });
    });
  });
});
