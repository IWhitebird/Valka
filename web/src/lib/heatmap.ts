import type { ShardStats } from "@/api/types";

export type HeatmapMode = "owner" | "load" | "snapshot_age" | "dirty";

export const OWNER_PALETTE = [
  "#6366f1",
  "#10b981",
  "#f59e0b",
  "#ec4899",
  "#06b6d4",
  "#84cc16",
  "#a855f7",
  "#f97316",
];

export const COLOR_EMPTY = "#18181b";
export const COLOR_IDLE = "#27272a";
export const COLOR_UNOWNED = "#7f1d1d";
export const COLOR_NEVER_SNAPSHOTTED = "#b45309";

/** Stable index per owner so the same node keeps the same colour between refreshes. */
export function ownerIndexOf(shards: ShardStats[]): Map<string, number> {
  const m = new Map<string, number>();
  for (const s of shards) {
    if (s.owner && !m.has(s.owner)) m.set(s.owner, m.size);
  }
  return m;
}

/** Colour for one heatmap cell. Pure, so the legend and tests share the exact mapping. */
export function cellColor(
  s: ShardStats | undefined,
  mode: HeatmapMode,
  ownerIndex: Map<string, number>,
  nowMs: number,
): string {
  if (!s) return COLOR_EMPTY;
  if (s.owner === null) return COLOR_UNOWNED;
  switch (mode) {
    case "owner": {
      const base = OWNER_PALETTE[(ownerIndex.get(s.owner) ?? 0) % OWNER_PALETTE.length];
      return s.tasks > 0 ? base : `${base}55`;
    }
    case "load": {
      const active = s.pending + s.running + s.retry;
      if (active === 0) return COLOR_IDLE;
      const t = Math.min(1, Math.log10(active + 1) / 3); // 1 → 0.1, 1000 → 1
      return `rgb(${Math.round(60 + 195 * t)}, ${Math.round(140 - 80 * t)}, ${Math.round(220 - 200 * t)})`;
    }
    case "snapshot_age": {
      if (s.shard_seq === 0) return COLOR_IDLE;
      if (!s.snapshot_at) return COLOR_NEVER_SNAPSHOTTED;
      const ageMin = (nowMs - new Date(s.snapshot_at).getTime()) / 60_000;
      const t = Math.min(1, Math.max(0, ageMin / 120));
      return `rgb(${Math.round(16 + 200 * t)}, ${Math.round(185 - 120 * t)}, ${Math.round(129 - 100 * t)})`;
    }
    case "dirty": {
      if (s.records_since_snapshot === 0) return COLOR_IDLE;
      const t = Math.min(1, Math.log10(s.records_since_snapshot + 1) / 4);
      return `rgb(${Math.round(80 + 170 * t)}, ${Math.round(160 - 100 * t)}, 60)`;
    }
  }
}

/** Grid geometry shared by the canvas painter and the hit test. */
export const GRID = 64;

export function shardAtPoint(x: number, y: number, size: number): number {
  if (size <= 0) return 0;
  const cell = size / GRID;
  const col = Math.min(GRID - 1, Math.max(0, Math.floor(x / cell)));
  const row = Math.min(GRID - 1, Math.max(0, Math.floor(y / cell)));
  return row * GRID + col;
}
