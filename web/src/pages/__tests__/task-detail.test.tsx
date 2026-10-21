import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskDetailPage } from "../task-detail";

const TASK_ID = "0192f3a4-aaaa-7bbb-8ccc-dddddddddddd";

const task = {
  id: TASK_ID,
  queue_name: "emails",
  task_name: "send_email",
  status: "RETRY",
  priority: 0,
  max_retries: 3,
  attempt_count: 1,
  timeout_seconds: 300,
  idempotency_key: null,
  input: null,
  metadata: {},
  output: null,
  error_message: "smtp down",
  scheduled_at: null,
  created_at: "2026-10-22T10:00:00Z",
  updated_at: "2026-10-22T10:00:05Z",
};

function renderPage(taskResponse: () => Response) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string) => {
      if (path === `/api/v1/tasks/${TASK_ID}`) return taskResponse();
      return new Response("[]", { status: 200 });
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[`/tasks/${TASK_ID}`]}>
        <Routes>
          <Route path="/tasks/:taskId" element={<TaskDetailPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const json = (status: number, body: unknown) => () =>
  new Response(JSON.stringify(body), { status });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("TaskDetailPage", () => {
  it("says not found only when the server says 404", async () => {
    renderPage(json(404, { error: "Task not found", code: "NOT_FOUND" }));
    expect(await screen.findByText("Task not found")).toBeInTheDocument();
  });

  it("shows any other failure as a failure", async () => {
    renderPage(json(503, { error: "owner node unavailable", code: "UNAVAILABLE" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not load task: owner node unavailable",
    );
    expect(screen.queryByText("Task not found")).not.toBeInTheDocument();
  });

  it("offers Cancel and signals for a task waiting to retry", async () => {
    renderPage(json(200, task));
    expect(await screen.findByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(screen.getByText("Send Signal")).toBeInTheDocument();
  });
});
