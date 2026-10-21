// Cluster-wide answers (task lists, workers, dead letters) are gathered from every node. When a
// node does not answer in time the server still replies, with `X-Valka-Partial: true;
// missing=a,b`. This module remembers which endpoints last answered without some node's part,
// so one banner can say the page may be incomplete until a later answer is whole again.

export const PARTIAL_HEADER = "x-valka-partial";

export interface PartialInfo {
  partial: boolean;
  /** Nodes that did not answer (may be empty when the server does not name them). */
  missing: string[];
}

/** Parse an `X-Valka-Partial` value; null when the response carries no such header. */
export function parsePartialHeader(value: string | null): PartialInfo | null {
  if (value === null) return null;
  const v = value.trim();
  if (!v.toLowerCase().startsWith("true")) return { partial: false, missing: [] };
  const m = /missing=([^;]*)/.exec(v);
  const missing = m
    ? m[1]
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean)
    : [];
  return { partial: true, missing };
}

const byEndpoint = new Map<string, string[]>();
const listeners = new Set<() => void>();
/** null: every remembered answer was complete; otherwise the nodes missing from any of them. */
let current: string[] | null = null;

function recompute() {
  if (byEndpoint.size === 0) {
    current = null;
  } else {
    const nodes = new Set<string>();
    for (const missing of byEndpoint.values()) for (const n of missing) nodes.add(n);
    current = [...nodes].sort();
  }
  for (const l of listeners) l();
}

export const partialAnswers = {
  /** Remember how `endpoint` (a path without its query) last answered. */
  record(endpoint: string, info: PartialInfo): void {
    const had = byEndpoint.has(endpoint);
    if (info.partial) {
      byEndpoint.set(endpoint, info.missing);
      recompute();
    } else if (had) {
      byEndpoint.delete(endpoint);
      recompute();
    }
  },
  /** Forget everything (a new page asks its own questions). */
  clear(): void {
    if (byEndpoint.size === 0) return;
    byEndpoint.clear();
    recompute();
  },
  /** Stable between changes, for `useSyncExternalStore`. */
  snapshot(): string[] | null {
    return current;
  },
  subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  },
};
