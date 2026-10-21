import { describe, expect, it } from "vitest";
import { isTerminalStatus, localDateTimeToIso } from "../utils";

describe("isTerminalStatus", () => {
  it("matches the server: only ended tasks are terminal, RETRY is not", () => {
    for (const s of ["COMPLETED", "FAILED", "DEAD_LETTER", "CANCELLED"]) {
      expect(isTerminalStatus(s)).toBe(true);
    }
    for (const s of ["PENDING", "DISPATCHING", "RUNNING", "RETRY"]) {
      expect(isTerminalStatus(s)).toBe(false);
    }
  });
});

describe("localDateTimeToIso", () => {
  it("turns a datetime-local value into the UTC instant it names", () => {
    expect(localDateTimeToIso("2026-10-22T14:30")).toBe(new Date(2026, 9, 22, 14, 30).toISOString());
  });
  it("sends nothing for an empty or invalid value", () => {
    expect(localDateTimeToIso("")).toBeUndefined();
    expect(localDateTimeToIso("not a date")).toBeUndefined();
  });
});
