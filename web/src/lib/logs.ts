import type { TaskLog } from "@/api/types";

// A run's log is read with `after_id` paging: line ids are 1-based positions in the log, so
// the viewer asks only for lines after the newest one it has. It keeps a window of the newest
// lines; older ones can be loaded back on demand.

/** Lines the viewer keeps (the newest ones) unless earlier lines were asked for. */
export const LOG_WINDOW = 2000;
/** Lines per request while catching up with a log (the server allows up to 10000). */
export const LOG_PAGE = 5000;
/** Requests per refresh at most; a longer backlog continues on the next refresh. */
export const LOG_MAX_PAGES = 20;
/** Lines per "load earlier". */
export const LOG_EARLIER_PAGE = 1000;

export interface LogWindow {
  /** Lines shown, oldest first. */
  lines: TaskLog[];
  /** Id of the newest line seen; 0 before the first line. */
  lastId: number;
  /** The last refresh reached the end of the log as it was then. */
  caughtUp: boolean;
  /** Lines the last refresh added. */
  added: number;
}

export const EMPTY_LOG_WINDOW: LogWindow = { lines: [], lastId: 0, caughtUp: false, added: 0 };

/** Lines before the first one shown: they exist but are not loaded. */
export function earlierCount(w: LogWindow): number {
  return w.lines.length > 0 ? w.lines[0].id - 1 : 0;
}

/**
 * Add newer lines (ignoring ones already seen). Keeps at most `cap` lines, or the current
 * count when earlier lines were loaded on purpose, dropping the oldest.
 */
export function appendLogs(w: LogWindow, fresh: TaskLog[], cap = LOG_WINDOW): LogWindow {
  const newer = fresh.filter((l) => l.id > w.lastId);
  if (newer.length === 0) return w;
  const keep = Math.max(cap, w.lines.length);
  const all = w.lines.concat(newer);
  return {
    ...w,
    lines: all.length > keep ? all.slice(all.length - keep) : all,
    lastId: newer.reduce((m, l) => Math.max(m, l.id), w.lastId),
  };
}

/** The request for up to `page` lines just before the first one shown; null if none. */
export function earlierRequest(
  w: LogWindow,
  page = LOG_EARLIER_PAGE,
): { afterId: number; limit: number } | null {
  const before = earlierCount(w);
  if (before === 0) return null;
  const limit = Math.min(page, before);
  return { afterId: before - limit, limit };
}

/** Put earlier lines in front (ignoring any already shown). */
export function prependLogs(w: LogWindow, earlier: TaskLog[]): LogWindow {
  const first = w.lines[0]?.id ?? Number.POSITIVE_INFINITY;
  const older = earlier.filter((l) => l.id < first);
  return older.length === 0 ? w : { ...w, lines: older.concat(w.lines) };
}

/**
 * Fetch every line after `from.lastId`, page by page, up to `maxPages` requests. `fetchPage`
 * gets the `after_id` and `limit` to ask for.
 */
export async function readNewLogs(
  from: LogWindow,
  fetchPage: (afterId: number, limit: number) => Promise<TaskLog[]>,
  { page = LOG_PAGE, maxPages = LOG_MAX_PAGES, cap = LOG_WINDOW } = {},
): Promise<LogWindow> {
  let w: LogWindow = { ...from, caughtUp: false, added: 0 };
  for (let i = 0; i < maxPages; i++) {
    const lines = await fetchPage(w.lastId, page);
    const next = appendLogs(w, lines, cap);
    w = { ...next, added: w.added + (next.lastId - w.lastId) };
    if (lines.length < page) return { ...w, caughtUp: true };
  }
  return w;
}

/** Whether a log needs no more polling: its run is over and a refresh found nothing new. */
export function logsSettled(w: LogWindow | undefined, runFinished: boolean): boolean {
  return runFinished && w !== undefined && w.caughtUp && w.added === 0;
}
