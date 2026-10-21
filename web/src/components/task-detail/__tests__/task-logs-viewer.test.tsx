import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskLogsViewer } from "../task-logs-viewer";

/** A run log of `total` lines served like `GET .../runs/{run}/logs?after_id&limit`. */
function serveLog(total: number) {
  return vi.fn(async (path: string) => {
    const url = new URL(path, "http://localhost");
    const after = Number(url.searchParams.get("after_id") ?? 0);
    const limit = Number(url.searchParams.get("limit") ?? 1000);
    const lines = [];
    for (let id = after + 1; id <= Math.min(total, after + limit); id++) {
      lines.push({
        id,
        task_run_id: "run-1",
        level: "INFO",
        message: `line ${id}`,
        timestamp_ms: 1_700_000_000_000,
        metadata: null,
      });
    }
    return new Response(JSON.stringify(lines), { status: 200 });
  });
}

function renderViewer() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <TaskLogsViewer taskId="task-1" runId="run-1" runFinished />
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("TaskLogsViewer", () => {
  it("shows the end of a long log and loads earlier lines on demand", async () => {
    const fetchMock = serveLog(2500);
    vi.stubGlobal("fetch", fetchMock);
    renderViewer();

    expect(await screen.findByText("line 2500")).toBeInTheDocument();
    expect(screen.queryByText("line 500")).not.toBeInTheDocument();
    expect(screen.getByText("500 earlier lines not shown")).toBeInTheDocument();
    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/v1/tasks/task-1/runs/run-1/logs?after_id=0&limit=5000",
    );

    fireEvent.click(screen.getByRole("button", { name: "Load earlier" }));
    expect(await screen.findByText("line 1")).toBeInTheDocument();
    expect(screen.queryByText(/earlier lines? not shown/)).not.toBeInTheDocument();
    expect(fetchMock.mock.calls.at(-1)?.[0]).toBe(
      "/api/v1/tasks/task-1/runs/run-1/logs?after_id=0&limit=500",
    );
  });

  it("reports a failure instead of an empty log", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ error: "bucket unreachable" }), { status: 500 })),
    );
    renderViewer();
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("Could not load logs: bucket unreachable"),
    );
    expect(screen.queryByText("No logs available")).not.toBeInTheDocument();
  });
});
