import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskCreateDialog } from "../task-create-dialog";

function renderDialog() {
  const onClose = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <TaskCreateDialog open onClose={onClose} />
    </QueryClientProvider>,
  );
  return onClose;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("TaskCreateDialog", () => {
  it("keeps the picked schedule visible and sends it as a UTC instant", async () => {
    const fetchMock = vi.fn<(path: string, init?: RequestInit) => Promise<Response>>(
      async () => new Response("{}", { status: 201 }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const onClose = renderDialog();

    fireEvent.change(screen.getByLabelText("Queue Name *"), { target: { value: "emails" } });
    fireEvent.change(screen.getByLabelText("Task Name *"), { target: { value: "send" } });
    const schedule = screen.getByLabelText("Schedule At (local time)");
    fireEvent.change(schedule, { target: { value: "2026-10-22T14:30" } });
    expect(schedule).toHaveValue("2026-10-22T14:30");

    fireEvent.click(screen.getByRole("button", { name: "Create Task" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const body = JSON.parse(String(fetchMock.mock.calls[0][1]?.body));
    expect(body.scheduled_at).toBe(new Date(2026, 9, 22, 14, 30).toISOString());
  });

  it("forgets what was typed when it is cancelled", () => {
    const onClose = renderDialog();
    const schedule = screen.getByLabelText("Schedule At (local time)");
    fireEvent.change(schedule, { target: { value: "2026-10-22T14:30" } });
    fireEvent.change(screen.getByLabelText("Queue Name *"), { target: { value: "emails" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalled();
    expect(schedule).toHaveValue("");
    expect(screen.getByLabelText("Queue Name *")).toHaveValue("");
  });
});
