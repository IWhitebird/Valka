import { describe, expect, it, vi } from "vitest";
import type { TaskLog } from "@/api/types";
import {
  EMPTY_LOG_WINDOW,
  appendLogs,
  earlierCount,
  earlierRequest,
  logsSettled,
  prependLogs,
  readNewLogs,
  type LogWindow,
} from "../logs";

function line(id: number): TaskLog {
  return {
    id,
    task_run_id: "run-1",
    level: "INFO",
    message: `line ${id}`,
    timestamp_ms: 1_700_000_000_000 + id,
    metadata: null,
  };
}

function lines(from: number, to: number): TaskLog[] {
  return Array.from({ length: to - from + 1 }, (_, i) => line(from + i));
}

/** A server log of `total` lines answering `after_id` / `limit` like the REST handler. */
function serverLog(total: number) {
  return vi.fn(async (afterId: number, limit: number) =>
    lines(afterId + 1, Math.min(total, afterId + limit)),
  );
}

describe("appendLogs", () => {
  it("adds only unseen lines and keeps the newest `cap`", () => {
    let w = appendLogs(EMPTY_LOG_WINDOW, lines(1, 3), 4);
    expect(w.lines.map((l) => l.id)).toEqual([1, 2, 3]);
    expect(w.lastId).toBe(3);
    w = appendLogs(w, lines(2, 6), 4);
    expect(w.lines.map((l) => l.id)).toEqual([3, 4, 5, 6]);
    expect(w.lastId).toBe(6);
    expect(earlierCount(w)).toBe(2);
    expect(appendLogs(w, lines(1, 6), 4)).toBe(w);
  });

  it("keeps earlier lines loaded on purpose, dropping the oldest as new ones come", () => {
    const w: LogWindow = { ...EMPTY_LOG_WINDOW, lines: lines(1, 6), lastId: 6 };
    const next = appendLogs(w, lines(7, 8), 4);
    expect(next.lines.map((l) => l.id)).toEqual([3, 4, 5, 6, 7, 8]);
  });
});

describe("earlier lines", () => {
  it("asks for the page just before the first line shown", () => {
    const w: LogWindow = { ...EMPTY_LOG_WINDOW, lines: lines(2501, 3000), lastId: 3000 };
    expect(earlierRequest(w, 1000)).toEqual({ afterId: 1500, limit: 1000 });
    const partial: LogWindow = { ...w, lines: lines(301, 3000) };
    expect(earlierRequest(partial, 1000)).toEqual({ afterId: 0, limit: 300 });
    expect(earlierRequest({ ...w, lines: lines(1, 3000) })).toBeNull();
    expect(earlierRequest(EMPTY_LOG_WINDOW)).toBeNull();
  });

  it("puts earlier lines in front without duplicates", () => {
    const w: LogWindow = { ...EMPTY_LOG_WINDOW, lines: lines(5, 6), lastId: 6 };
    expect(prependLogs(w, lines(3, 5)).lines.map((l) => l.id)).toEqual([3, 4, 5, 6]);
    expect(prependLogs(w, [])).toBe(w);
  });
});

describe("readNewLogs", () => {
  it("catches up page by page and keeps the tail of a long log", async () => {
    const fetchPage = serverLog(12);
    const w = await readNewLogs(EMPTY_LOG_WINDOW, fetchPage, { page: 5, cap: 4 });
    expect(fetchPage.mock.calls).toEqual([
      [0, 5],
      [5, 5],
      [10, 5],
    ]);
    expect(w.lines.map((l) => l.id)).toEqual([9, 10, 11, 12]);
    expect(w).toMatchObject({ lastId: 12, caughtUp: true, added: 12 });
  });

  it("asks only for lines after the newest one held", async () => {
    const fetchPage = serverLog(14);
    const start: LogWindow = { lines: lines(9, 12), lastId: 12, caughtUp: true, added: 3 };
    const w = await readNewLogs(start, fetchPage, { page: 5, cap: 4 });
    expect(fetchPage.mock.calls).toEqual([[12, 5]]);
    expect(w).toMatchObject({ lastId: 14, caughtUp: true, added: 2 });
  });

  it("stops after maxPages and says it has not caught up", async () => {
    const fetchPage = serverLog(100);
    const w = await readNewLogs(EMPTY_LOG_WINDOW, fetchPage, { page: 5, maxPages: 2, cap: 4 });
    expect(fetchPage).toHaveBeenCalledTimes(2);
    expect(w).toMatchObject({ lastId: 10, caughtUp: false });
  });
});

describe("logsSettled", () => {
  it("stops polling only once the run is over and a refresh found nothing new", () => {
    const quiet: LogWindow = { ...EMPTY_LOG_WINDOW, caughtUp: true, added: 0 };
    expect(logsSettled(quiet, true)).toBe(true);
    expect(logsSettled(quiet, false)).toBe(false);
    expect(logsSettled({ ...quiet, added: 3 }, true)).toBe(false);
    expect(logsSettled({ ...quiet, caughtUp: false }, true)).toBe(false);
    expect(logsSettled(undefined, true)).toBe(false);
  });
});
