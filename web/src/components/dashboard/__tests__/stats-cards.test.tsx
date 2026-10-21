import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { StatsCards } from "../stats-cards";

function card(title: string): HTMLElement {
  return screen.getByText(title).parentElement as HTMLElement;
}

describe("StatsCards", () => {
  it("shows the cluster's task counts, not a sample of tasks", () => {
    render(
      <StatsCards
        isLoading={false}
        counts={{
          pending: 12_000,
          running: 30,
          retry: 4,
          completed: 7_500,
          failed: 2,
          dead_letter: 3,
          cancelled: 1,
          total: 19_540,
        }}
      />,
    );
    expect(card("Total Tasks")).toHaveTextContent((19_540).toLocaleString());
    expect(card("Pending")).toHaveTextContent((12_000).toLocaleString());
    expect(card("Running")).toHaveTextContent("30");
    expect(card("Failed")).toHaveTextContent("5");
  });

  it("says when the counts could not be loaded", () => {
    render(
      <StatsCards isLoading={false} counts={undefined} error={new ApiError(503, "no answer")} />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("Could not load task counts: no answer");
  });
});
