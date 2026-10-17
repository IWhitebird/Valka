import { useEffect, useMemo, useRef, useState } from "react";
import type { ShardStats } from "@/api/types";
import { snapshotName } from "@/lib/cluster";
import { type HeatmapMode, cellColor, gridDims, ownerIndexOf, shardAtPoint } from "@/lib/heatmap";

const WIDTH = 768;

interface ShardHeatmapProps {
  shards: ShardStats[];
  numShards: number;
  mode: HeatmapMode;
  selected: number | null;
  onSelect: (shard: number | null) => void;
  nowMs: number;
}

export function ShardHeatmap({ shards, numShards, mode, selected, onSelect, nowMs }: ShardHeatmapProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [hover, setHover] = useState<{ shard: number; x: number; y: number } | null>(null);
  const { cols, rows } = gridDims(numShards);
  const cell = WIDTH / cols;
  const height = Math.round(rows * cell);
  const byShard = useMemo(() => {
    const arr: (ShardStats | undefined)[] = new Array(numShards);
    for (const s of shards) arr[s.shard] = s;
    return arr;
  }, [shards, numShards]);
  const ownerIndex = useMemo(() => ownerIndexOf(shards), [shards]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    const gap = cell > 4 ? 1 : 0;
    for (let i = 0; i < numShards; i++) {
      const x = (i % cols) * cell;
      const y = Math.floor(i / cols) * cell;
      ctx.fillStyle = cellColor(byShard[i], mode, ownerIndex, nowMs);
      ctx.fillRect(x, y, cell - gap, cell - gap);
    }
    const ring = (i: number, color: string) => {
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.strokeRect((i % cols) * cell - 0.5, Math.floor(i / cols) * cell - 0.5, cell, cell);
    };
    if (hover) ring(hover.shard, "#ffffffaa");
    if (selected !== null) ring(selected, "#ffffff");
  }, [byShard, mode, ownerIndex, hover, selected, nowMs, numShards, cols, cell]);

  function shardAt(e: React.MouseEvent<HTMLCanvasElement>): number {
    const rect = e.currentTarget.getBoundingClientRect();
    // The canvas is scaled to its box: map back to canvas pixels' grid.
    return shardAtPoint(e.clientX - rect.left, e.clientY - rect.top, rect.width, numShards);
  }

  const hovered = hover ? byShard[hover.shard] : undefined;

  return (
    <div className="relative">
      <canvas
        ref={canvasRef}
        width={WIDTH}
        height={height}
        style={{ aspectRatio: `${WIDTH} / ${height}` }}
        className="w-full max-w-[768px] cursor-crosshair rounded-md border bg-background"
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
              <div>
                owner {hovered.owner ?? <span className="text-red-400">none</span>}
                {hovered.owner && ` · gen ${hovered.gen} · ${hovered.phase}`}
              </div>
              <div>
                {hovered.tasks} tasks · {hovered.pending} pending · {hovered.running} running
              </div>
              <div>
                {hovered.records_since_snapshot} records since snapshot
                {hovered.snapshot_key ? ` (snapshot ${snapshotName(hovered.snapshot_key)})` : ", never snapshotted"}
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
