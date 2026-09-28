import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { TaskCheckpoint } from "@/api/types";
import { formatJsonValue } from "@/lib/utils";
import { TaskCheckpointsTable } from "../task-checkpoints-table";

function checkpoint(overrides: Partial<TaskCheckpoint> = {}): TaskCheckpoint {
  return {
    task_id: "task-1",
    step: "fetch",
    output: { rows: 3 },
    run_id: "0192f3a4-aaaa-7bbb-8ccc-dddddddddddd",
    attempt_number: 1,
    created_at: "2026-09-24T10:00:00Z",
    ...overrides,
  };
}

describe("TaskCheckpointsTable", () => {
  it("renders one row per step in order with attempt and output", () => {
    render(
      <TaskCheckpointsTable
        checkpoints={[
          checkpoint(),
          checkpoint({ step: "transform", attempt_number: 2, output: [1, 2] }),
          checkpoint({ step: "notify", attempt_number: 2, output: null }),
        ]}
      />,
    );
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    expect(within(rows[0]).getByText("fetch")).toBeInTheDocument();
    expect(within(rows[0]).getByText("#1")).toBeInTheDocument();
    expect(within(rows[0]).getByText('{"rows":3}')).toBeInTheDocument();
    expect(within(rows[0]).getByText("0192f3a4")).toBeInTheDocument();
    expect(within(rows[1]).getByText("transform")).toBeInTheDocument();
    expect(within(rows[1]).getByText("#2")).toBeInTheDocument();
    expect(within(rows[1]).getByText("[1,2]")).toBeInTheDocument();
    expect(within(rows[2]).getByText("--")).toBeInTheDocument();
  });
});

describe("formatJsonValue", () => {
  it("serialises values compactly and dashes empty outputs", () => {
    expect(formatJsonValue({ a: "b" })).toBe('{"a":"b"}');
    expect(formatJsonValue("done")).toBe('"done"');
    expect(formatJsonValue(0)).toBe("0");
    expect(formatJsonValue(null)).toBe("--");
    expect(formatJsonValue(undefined)).toBe("--");
  });
});
