import type {
  ClusterHealth,
  ClusterNode,
  ClusterNodeStats,
  NodeStatus,
  TaskCounts,
} from "@/api/types";
import { hasStats } from "@/api/types";

/** Shard count of a new bucket; a cluster's real count is `ClusterOverview.num_shards`. */
export const DEFAULT_NUM_SHARDS = 4096;

/**
 * The shard a task lives in is embedded in the low 16 bits of its UUIDv7, masked to the
 * cluster's shard count, a power of two (see `valka_core::shard::embed_shard`). Returns null
 * for non-UUID ids.
 */
export function shardOfTaskId(taskId: string, numShards = DEFAULT_NUM_SHARDS): number | null {
  const hex = taskId.replace(/-/g, "");
  if (!/^[0-9a-fA-F]{32}$/.test(hex)) return null;
  return parseInt(hex.slice(28, 32), 16) & (numShards - 1);
}

/** Sum a per-node figure over the nodes that answered with their stats. */
export function sumStats(nodes: ClusterNode[], f: (n: ClusterNodeStats) => number): number {
  return nodes.reduce((s, n) => s + (hasStats(n) ? f(n) : 0), 0);
}

const TASK_COUNT_KEYS = [
  "pending",
  "running",
  "retry",
  "completed",
  "failed",
  "dead_letter",
  "cancelled",
  "total",
] as const satisfies readonly (keyof TaskCounts)[];

/**
 * Tasks held in RAM across the cluster: every answering node's own counts, summed (each task
 * lives on exactly one node, its shard's owner).
 */
export function sumTaskCounts(nodes: ClusterNode[]): TaskCounts {
  const sum: TaskCounts = {
    pending: 0,
    running: 0,
    retry: 0,
    completed: 0,
    failed: 0,
    dead_letter: 0,
    cancelled: 0,
    total: 0,
  };
  for (const n of nodes) {
    if (!hasStats(n)) continue;
    for (const k of TASK_COUNT_KEYS) sum[k] += n.tasks[k];
  }
  return sum;
}

/** The tail of a snapshot key: `{gen}-{seq}-{ptr}.snap`. */
export function snapshotName(key: string): string {
  return key.slice(key.lastIndexOf("/") + 1);
}

/** "0–1365, 2731–4095" style summary of a sorted list of shard ids. */
export function compressRanges(shards: number[]): string {
  if (shards.length === 0) return "none";
  const sorted = [...shards].sort((a, b) => a - b);
  const parts: string[] = [];
  let start = sorted[0];
  let prev = sorted[0];
  for (let i = 1; i <= sorted.length; i++) {
    const cur = sorted[i];
    if (cur === prev + 1) {
      prev = cur;
      continue;
    }
    parts.push(start === prev ? `${start}` : `${start}–${prev}`);
    start = cur;
    prev = cur;
  }
  return parts.join(", ");
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(2) : v < 100 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function formatDurationMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return "—";
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
  const m = s / 60;
  if (m < 60) return `${Math.round(m)} min`;
  const h = m / 60;
  if (h < 48) return `${h.toFixed(1)} h`;
  return `${Math.round(h / 24)} d`;
}

export function formatDurationSecs(secs: number | null | undefined): string {
  return secs === null || secs === undefined ? "—" : formatDurationMs(secs * 1000);
}

export function nodeStatusDot(status: NodeStatus): string {
  switch (status) {
    case "alive":
      return "bg-green-400";
    case "draining":
      return "bg-amber-400";
    case "suspect":
      return "bg-orange-400";
    case "fenced":
    case "poisoned":
    case "dead":
      return "bg-red-400";
    default:
      return "bg-zinc-400";
  }
}

export function healthTone(status: ClusterHealth["status"]): {
  badge: string;
  bar: string;
  label: string;
} {
  switch (status) {
    case "ok":
      return {
        badge: "bg-green-500/10 text-green-400 border-green-500/20",
        bar: "border-green-500/30 bg-green-500/5",
        label: "Healthy",
      };
    case "degraded":
      return {
        badge: "bg-amber-500/10 text-amber-400 border-amber-500/20",
        bar: "border-amber-500/30 bg-amber-500/5",
        label: "Degraded",
      };
    case "critical":
      return {
        badge: "bg-red-500/10 text-red-400 border-red-500/20",
        bar: "border-red-500/30 bg-red-500/5",
        label: "Critical",
      };
  }
}

/** Human summary of what is wrong, or null when healthy. */
export function healthProblems(
  health: ClusterHealth,
  numShards: number,
): string[] {
  const problems: string[] = [];
  if (health.poisoned_nodes.length > 0) {
    problems.push(
      `WAL writer poisoned on ${health.poisoned_nodes.join(", ")} — the node cannot commit and must restart`,
    );
  }
  if (health.unowned_shards === null) {
    problems.push("Some nodes did not answer: shard ownership is unknown");
  } else if (health.unowned_shards > 0) {
    problems.push(
      `${health.unowned_shards} of ${numShards} shards have no live owner`,
    );
  }
  if (health.suspect_nodes.length > 0) {
    problems.push(`${health.suspect_nodes.join(", ")} missed heartbeats`);
  }
  return problems;
}
