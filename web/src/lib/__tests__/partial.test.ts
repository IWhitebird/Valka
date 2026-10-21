import { afterEach, describe, expect, it, vi } from "vitest";
import { parsePartialHeader, partialAnswers } from "../partial";

afterEach(() => {
  partialAnswers.clear();
});

describe("parsePartialHeader", () => {
  it("reads the server's X-Valka-Partial values", () => {
    expect(parsePartialHeader(null)).toBeNull();
    expect(parsePartialHeader("false")).toEqual({ partial: false, missing: [] });
    expect(parsePartialHeader("true; missing=node-b,node-c")).toEqual({
      partial: true,
      missing: ["node-b", "node-c"],
    });
    expect(parsePartialHeader("true")).toEqual({ partial: true, missing: [] });
  });
});

describe("partialAnswers", () => {
  it("is partial while any endpoint's last answer was, and clears when it is whole again", () => {
    const listener = vi.fn();
    const unsubscribe = partialAnswers.subscribe(listener);
    expect(partialAnswers.snapshot()).toBeNull();

    partialAnswers.record("/api/v1/workers", { partial: true, missing: ["b"] });
    partialAnswers.record("/api/v1/tasks", { partial: true, missing: ["c", "b"] });
    expect(partialAnswers.snapshot()).toEqual(["b", "c"]);

    partialAnswers.record("/api/v1/tasks", { partial: false, missing: [] });
    expect(partialAnswers.snapshot()).toEqual(["b"]);
    partialAnswers.record("/api/v1/workers", { partial: false, missing: [] });
    expect(partialAnswers.snapshot()).toBeNull();
    expect(listener).toHaveBeenCalledTimes(4);

    // A complete answer for an endpoint that was never partial changes nothing.
    partialAnswers.record("/api/v1/dead-letters", { partial: false, missing: [] });
    expect(listener).toHaveBeenCalledTimes(4);
    unsubscribe();
  });

  it("keeps the snapshot stable between changes and forgets everything on clear", () => {
    partialAnswers.record("/api/v1/workers", { partial: true, missing: [] });
    const first = partialAnswers.snapshot();
    expect(first).toEqual([]);
    expect(partialAnswers.snapshot()).toBe(first);
    partialAnswers.clear();
    expect(partialAnswers.snapshot()).toBeNull();
  });
});
