import { describe, expect, it } from "vitest";
import type { ShardStats } from "@/api/types";
import {
  COLOR_EMPTY,
  COLOR_IDLE,
  COLOR_NEVER_SNAPSHOTTED,
  COLOR_UNOWNED,
  OWNER_PALETTE,
  cellColor,
  ownerIndexOf,
  shardAtPoint,
} from "../heatmap";

function shard(overrides: Partial<ShardStats> = {}): ShardStats {
  return {
    shard: 1,
    owner: "a",
    epoch: 1,
    tasks: 0,
    pending: 0,
    running: 0,
    retry: 0,
    signals: 0,
    dead_letters: 0,
    shard_seq: 0,
    snapshot_seq: 0,
    snapshot_lsn: null,
    snapshot_at: null,
    records_since_snapshot: 0,
    dirty_since_lsn: null,
    ...overrides,
  };
}

const now = Date.parse("2026-09-04T12:00:00Z");

describe("cellColor", () => {
  const idx = ownerIndexOf([shard({ owner: "a" }), shard({ owner: "b" })]);

  it("marks missing and unowned shards distinctly in every mode", () => {
    for (const mode of ["owner", "load", "snapshot_age", "dirty"] as const) {
      expect(cellColor(undefined, mode, idx, now)).toBe(COLOR_EMPTY);
      expect(cellColor(shard({ owner: null }), mode, idx, now)).toBe(COLOR_UNOWNED);
    }
  });

  it("colours by owner, dimming shards without tasks", () => {
    expect(cellColor(shard({ owner: "a", tasks: 3 }), "owner", idx, now)).toBe(OWNER_PALETTE[0]);
    expect(cellColor(shard({ owner: "b", tasks: 3 }), "owner", idx, now)).toBe(OWNER_PALETTE[1]);
    expect(cellColor(shard({ owner: "a", tasks: 0 }), "owner", idx, now)).toBe(`${OWNER_PALETTE[0]}55`);
  });

  it("load: idle is neutral and heavier load is warmer", () => {
    expect(cellColor(shard(), "load", idx, now)).toBe(COLOR_IDLE);
    const light = cellColor(shard({ pending: 1 }), "load", idx, now);
    const heavy = cellColor(shard({ pending: 900 }), "load", idx, now);
    const red = (c: string) => Number(/rgb\((\d+)/.exec(c)![1]);
    expect(red(heavy)).toBeGreaterThan(red(light));
  });

  it("snapshot age: never snapshotted shards with history stand out", () => {
    expect(cellColor(shard({ shard_seq: 0 }), "snapshot_age", idx, now)).toBe(COLOR_IDLE);
    expect(cellColor(shard({ shard_seq: 5 }), "snapshot_age", idx, now)).toBe(COLOR_NEVER_SNAPSHOTTED);
    const fresh = cellColor(shard({ shard_seq: 5, snapshot_at: new Date(now - 60_000).toISOString() }), "snapshot_age", idx, now);
    const stale = cellColor(shard({ shard_seq: 5, snapshot_at: new Date(now - 6 * 3600_000).toISOString() }), "snapshot_age", idx, now);
    expect(fresh).not.toBe(stale);
    expect(stale).toBe("rgb(216, 65, 29)");
  });

  it("dirty: covered shards are neutral", () => {
    expect(cellColor(shard(), "dirty", idx, now)).toBe(COLOR_IDLE);
    expect(cellColor(shard({ records_since_snapshot: 10 }), "dirty", idx, now)).not.toBe(COLOR_IDLE);
  });
});

describe("shardAtPoint", () => {
  it("maps pixels to the 64×64 grid and clamps at the edges", () => {
    expect(shardAtPoint(0, 0, 640)).toBe(0);
    expect(shardAtPoint(15, 0, 640)).toBe(1);
    expect(shardAtPoint(0, 10, 640)).toBe(64);
    expect(shardAtPoint(639, 639, 640)).toBe(4095);
    expect(shardAtPoint(-5, 10_000, 640)).toBe(4032);
  });
});
