import type { ClusterHealth, NodeStatus } from "@/api/types";

/** Storage shards are fixed; mirrors `valka_core::NUM_SHARDS`. */
export const NUM_SHARDS = 4096;

/**
 * The shard a task lives in is embedded in the low 12 bits of its UUIDv7
 * (see `valka_core::shard::embed_shard`). Returns null for non-UUID ids.
 */
export function shardOfTaskId(taskId: string): number | null {
  const hex = taskId.replace(/-/g, "");
  if (!/^[0-9a-fA-F]{32}$/.test(hex)) return null;
  return parseInt(hex.slice(28, 32), 16) & 0x0fff;
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
  if (health.unowned_shards > 0) {
    problems.push(
      `${health.unowned_shards} of ${numShards} shards have no live owner`,
    );
  }
  if (health.suspect_nodes.length > 0) {
    problems.push(`${health.suspect_nodes.join(", ")} missed heartbeats`);
  }
  return problems;
}
