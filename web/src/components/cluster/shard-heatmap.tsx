import { useEffect, useMemo, useRef, useState } from "react";
import type { ShardStats } from "@/api/types";
import { NUM_SHARDS } from "@/lib/cluster";
import { GRID, type HeatmapMode, cellColor, ownerIndexOf, shardAtPoint } from "@/lib/heatmap";

interface ShardHeatmapProps {
  shards: ShardStats[];
  mode: HeatmapMode;
  selected: number | null;
  onSelect: (shard: number | null) => void;
  nowMs: number;
}

export function ShardHeatmap({ shards, mode, selected, onSelect, nowMs }: ShardHeatmapProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [hover, setHover] = useState<{ shard: number; x: number; y: number } | null>(null);
  const byShard = useMemo(() => {
    const arr: (ShardStats | undefined)[] = new Array(NUM_SHARDS);
    for (const s of shards) arr[s.shard] = s;
    return arr;
  }, [shards]);
  const ownerIndex = useMemo(() => ownerIndexOf(shards), [shards]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const size = canvas.width;
    const cell = size / GRID;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, size, size);
    for (let i = 0; i < NUM_SHARDS; i++) {
      const x = (i % GRID) * cell;
      const y = Math.floor(i / GRID) * cell;
      ctx.fillStyle = cellColor(byShard[i], mode, ownerIndex, nowMs);
      ctx.fillRect(x, y, cell - 1, cell - 1);
    }
    const ring = (i: number, color: string) => {
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.strokeRect((i % GRID) * cell - 0.5, Math.floor(i / GRID) * cell - 0.5, cell, cell);
    };
    if (hover) ring(hover.shard, "#ffffffaa");
    if (selected !== null) ring(selected, "#ffffff");
  }, [byShard, mode, ownerIndex, hover, selected, nowMs]);

  function shardAt(e: React.MouseEvent<HTMLCanvasElement>): number {
    const rect = e.currentTarget.getBoundingClientRect();
    return shardAtPoint(e.clientX - rect.left, e.clientY - rect.top, rect.width);
  }

  const hovered = hover ? byShard[hover.shard] : undefined;

  return (
    <div className="relative">
      <canvas
        ref={canvasRef}
        width={768}
        height={768}
        className="aspect-square w-full max-w-[768px] cursor-crosshair rounded-md border bg-background"
        onMouseMove={(e) => setHover({ shard: shardAt(e), x: e.nativeEvent.offsetX, y: e.nativeEvent.offsetY })}
        onMouseLeave={() => setHover(null)}
        onClick={(e) => {
          const s = shardAt(e);
          onSelect(selected === s ? null : s);
        }}
        aria-label="Shard map"
      />
      {hover && (
        <div
          className="pointer-events-none absolute z-10 rounded-md border bg-popover px-2.5 py-1.5 text-xs shadow-md"
          style={{ left: Math.min(hover.x + 12, 560), top: hover.y + 12 }}
        >
          <div className="font-mono font-medium">shard {hover.shard}</div>
          {hovered ? (
            <div className="mt-0.5 space-y-0.5 text-muted-foreground">
              <div>owner {hovered.owner ?? <span className="text-red-400">none</span>} · epoch {hovered.epoch}</div>
              <div>
                {hovered.tasks} tasks · {hovered.pending} pending · {hovered.running} running
              </div>
              <div>
                {hovered.records_since_snapshot} records since snapshot
                {hovered.snapshot_lsn ? ` (at ${hovered.snapshot_lsn})` : ", never snapshotted"}
              </div>
            </div>
          ) : (
            <div className="text-muted-foreground">no data</div>
          )}
        </div>
      )}
    </div>
  );
}
