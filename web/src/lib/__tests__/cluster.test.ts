import { describe, expect, it } from "vitest";
import {
  compressRanges,
  formatBytes,
  formatDurationMs,
  healthProblems,
  shardOfTaskId,
} from "../cluster";

describe("shardOfTaskId", () => {
  it("matches the server's embedded shard (low 12 bits of the uuid)", () => {
    // This task id was created by valka-server and its snapshot landed under snapshots/2634/.
    expect(shardOfTaskId("01a06ae4-e721-73a3-8dcb-b48ecb400a4a")).toBe(2634);
    expect(shardOfTaskId("00000000-0000-7000-8000-000000000fff")).toBe(4095);
    expect(shardOfTaskId("00000000-0000-7000-8000-00000000f000")).toBe(0);
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
});
