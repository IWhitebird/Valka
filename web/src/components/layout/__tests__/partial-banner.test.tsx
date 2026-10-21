import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { partialAnswers } from "@/lib/partial";
import { PartialBanner } from "../partial-banner";

afterEach(() => {
  partialAnswers.clear();
});

describe("PartialBanner", () => {
  it("appears while an answer is partial and goes once it is whole again", () => {
    render(<PartialBanner />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    act(() => partialAnswers.record("/api/v1/workers", { partial: true, missing: ["node-b"] }));
    expect(screen.getByRole("status")).toHaveTextContent(
      "Some nodes did not answer (node-b): this page may be incomplete.",
    );

    act(() => partialAnswers.record("/api/v1/workers", { partial: false, missing: [] }));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});
