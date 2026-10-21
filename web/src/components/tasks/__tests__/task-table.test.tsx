import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import type { Task } from "@/api/types";
import { TaskTable } from "../task-table";

function task(id: string, overrides: Partial<Task> = {}): Task {
  return {
    id,
    queue_name: "emails",
    task_name: "send_email",
    status: "PENDING",
    priority: 0,
    max_retries: 3,
    attempt_count: 0,
    timeout_seconds: 300,
    idempotency_key: null,
    input: null,
    metadata: null,
    output: null,
    error_message: null,
    scheduled_at: null,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    ...overrides,
  };
}

function renderTable(props: Partial<React.ComponentProps<typeof TaskTable>> = {}) {
  const onPageChange = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <TaskTable
          tasks={[]}
          isLoading={false}
          offset={0}
          limit={25}
          hasMore={false}
          onPageChange={onPageChange}
          {...props}
        />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return onPageChange;
}

describe("TaskTable", () => {
  it("links each task by a keyboard-reachable link and labels its icon buttons", () => {
    const id = "0192f3a4-aaaa-7bbb-8ccc-dddddddddddd";
    renderTable({ tasks: [task(id)] });
    const link = screen.getByRole("link", { name: `Open task send_email (${id})` });
    expect(link).toHaveAttribute("href", `/tasks/${id}`);
    expect(screen.getByRole("button", { name: "Delete task 0192f3a4" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled();
  });

  it("enables Next only when the server has a task past this page", () => {
    const tasks = [task("a"), task("b")];
    const onPageChange = renderTable({ tasks, limit: 2, hasMore: true });
    fireEvent.click(screen.getByRole("button", { name: "Next page" }));
    expect(onPageChange).toHaveBeenCalledWith(2);
  });

  it("keeps the way back from a page past the end", () => {
    const onPageChange = renderTable({ tasks: [], offset: 25 });
    expect(screen.getByText("No more tasks")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next page" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Previous page" }));
    expect(onPageChange).toHaveBeenCalledWith(0);
  });
});
