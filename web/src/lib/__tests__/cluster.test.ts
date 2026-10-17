import { describe, expect, it } from "vitest";
import {
  compressRanges,
  formatBytes,
  formatDurationMs,
  healthProblems,
  shardOfTaskId,
  snapshotName,
  sumStats,
} from "../cluster";
import type { ClusterNode } from "@/api/types";

describe("shardOfTaskId", () => {
  it("matches the server's embedded shard (low 16 bits, masked to the shard count)", () => {
    expect(shardOfTaskId("01a06ae4-e721-73a3-8dcb-b48ecb400a4a")).toBe(2634);
    expect(shardOfTaskId("00000000-0000-7000-8000-00000000ffff")).toBe(4095);
    expect(shardOfTaskId("00000000-0000-7000-8000-00000000ffff", 65536)).toBe(65535);
    expect(shardOfTaskId("00000000-0000-7000-8000-00000000ffff", 256)).toBe(255);
    expect(shardOfTaskId("00000000-0000-7000-8000-00000000f000")).toBe(0);
    expect(shardOfTaskId("00000000-0000-7000-8000-00000000f000", 65536)).toBe(0xf000);
  });
  it("rejects non-uuids", () => {
    expect(shardOfTaskId("not-a-uuid")).toBeNull();
    expect(shardOfTaskId("")).toBeNull();
  });
});

describe("compressRanges", () => {
  it("collapses contiguous runs", () => {
    expect(compressRanges([0, 1, 2, 3, 7, 9, 10])).toBe("0–3, 7, 9–10");
    expect(compressRanges([])).toBe("none");
    expect(compressRanges([5])).toBe("5");
  });
  it("sorts first", () => {
    expect(compressRanges([3, 1, 2])).toBe("1–3");
  });
});

describe("formatting", () => {
  it("formats bytes", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.50 KB");
    expect(formatBytes(4 * 1024 * 1024)).toBe("4.00 MB");
    expect(formatBytes(250 * 1024 * 1024)).toBe("250 MB");
  });
  it("formats durations", () => {
    expect(formatDurationMs(null)).toBe("—");
    expect(formatDurationMs(250)).toBe("250 ms");
    expect(formatDurationMs(4500)).toBe("4.5 s");
    expect(formatDurationMs(90_000)).toBe("2 min");
    expect(formatDurationMs(3 * 3600_000)).toBe("3.0 h");
  });
});

describe("snapshotName", () => {
  it("keeps the file name of a snapshot key", () => {
    expect(snapshotName("snapshots/3f/00042/000000000002-00000000000000000017-final.snap")).toBe(
      "000000000002-00000000000000000017-final.snap",
    );
  });
});

describe("sumStats", () => {
  it("skips nodes known only from their lease", () => {
    const answered = { node_id: "a", wal: { unflushed_records: 2 }, shards_owned: 10 } as unknown as ClusterNode;
    const lease = { node_id: "b", inc: 3, status: "dead", protocol: 2, unsplit: [] } as unknown as ClusterNode;
    expect(sumStats([answered, lease], (n) => n.shards_owned)).toBe(10);
  });
});

describe("healthProblems", () => {
  it("is empty when healthy", () => {
    expect(
      healthProblems(
        { status: "ok", unowned_shards: 0, poisoned_nodes: [], suspect_nodes: [] },
        4096,
      ),
    ).toEqual([]);
  });
  it("names each problem", () => {
    const p = healthProblems(
      { status: "critical", unowned_shards: 12, poisoned_nodes: ["a"], suspect_nodes: ["b"] },
      4096,
    );
    expect(p).toHaveLength(3);
    expect(p[0]).toContain("poisoned on a");
    expect(p[1]).toContain("12 of 4096");
    expect(p[2]).toContain("b missed heartbeats");
  });
  it("says ownership is unknown when a node did not answer", () => {
    const p = healthProblems(
      { status: "degraded", unowned_shards: null, poisoned_nodes: [], suspect_nodes: [], partial: true },
      4096,
    );
    expect(p).toEqual(["Some nodes did not answer: shard ownership is unknown"]);
  });
});
