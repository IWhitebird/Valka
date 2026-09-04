import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { X } from "lucide-react";
import type { TaskCounts } from "@/api/types";
import { ClusterTabs } from "@/components/cluster/cluster-tabs";
import { ShardHeatmap } from "@/components/cluster/shard-heatmap";
import { type HeatmapMode, cellColor, ownerIndexOf } from "@/lib/heatmap";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { useShard, useShards } from "@/hooks/use-cluster";
import { formatDate, formatRelative } from "@/lib/utils";

const MODES: { value: HeatmapMode; label: string }[] = [
  { value: "owner", label: "Owner" },
  { value: "load", label: "Pending load" },
  { value: "snapshot_age", label: "Snapshot age" },
  { value: "dirty", label: "Records since snapshot" },
];

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between border-b py-2 last:border-b-0">
      <span className="text-sm text-muted-foreground">{label}</span>
      <span className="font-mono text-sm">{value}</span>
    </div>
  );
}

function counts(c: TaskCounts): string {
  return `${c.pending}p · ${c.running}r · ${c.completed}c · ${c.failed + c.dead_letter}f`;
}

export function ClusterShardsPage() {
  const [params, setParams] = useSearchParams();
  const nodeFilter = params.get("node") ?? "";
  const dirtyOnly = params.get("dirty") === "true";
  const [mode, setMode] = useState<HeatmapMode>("owner");
  const [selected, setSelected] = useState<number | null>(null);
  const {
    data: shards = [],
    isLoading,
    dataUpdatedAt,
  } = useShards({
    node: nodeFilter || undefined,
    dirty: dirtyOnly || undefined,
  });
  const { data: detail } = useShard(selected);
  // Ages are measured against the time the shard data was fetched, keeping render pure.
  const nowMs = dataUpdatedAt || 0;
  const owners = useMemo(() => Array.from(ownerIndexOf(shards).keys()), [shards]);
  const ownerIndex = useMemo(() => ownerIndexOf(shards), [shards]);
  const summary = useMemo(() => {
    let withTasks = 0;
    let dirty = 0;
    let unowned = 0;
    for (const s of shards) {
      if (s.tasks > 0) withTasks++;
      if (s.records_since_snapshot > 0) dirty++;
      if (s.owner === null) unowned++;
    }
    return { withTasks, dirty, unowned };
  }, [shards]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">Shards</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          4096 fixed storage shards. Each has exactly one writer; snapshots and replay happen per shard.
        </p>
      </div>
      <ClusterTabs />

      <div className="flex flex-wrap items-center gap-3">
        <Select value={mode} onValueChange={(v) => setMode(v as HeatmapMode)}>
          <SelectTrigger className="w-56">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {MODES.map((m) => (
              <SelectItem key={m.value} value={m.value}>
                Colour by {m.label.toLowerCase()}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          variant={dirtyOnly ? "default" : "outline"}
          size="sm"
          onClick={() => {
            const next = new URLSearchParams(params);
            if (dirtyOnly) next.delete("dirty");
            else next.set("dirty", "true");
            setParams(next);
          }}
        >
          Only dirty
        </Button>
        {nodeFilter && (
          <Badge variant="outline" className="gap-1">
            node: {nodeFilter}
            <button
              onClick={() => {
                const next = new URLSearchParams(params);
                next.delete("node");
                setParams(next);
              }}
              aria-label="Clear node filter"
            >
              <X className="h-3 w-3" />
            </button>
          </Badge>
        )}
        <span className="ml-auto text-xs text-muted-foreground">
          {shards.length.toLocaleString()} shown · {summary.withTasks} with tasks · {summary.dirty} dirty
          {summary.unowned > 0 && <span className="text-red-400"> · {summary.unowned} unowned</span>}
        </span>
      </div>

      <div className="grid gap-6 lg:grid-cols-[minmax(0,768px)_minmax(280px,1fr)]">
        <div className="space-y-3">
          {isLoading ? (
            <Skeleton className="aspect-square w-full max-w-[768px]" />
          ) : (
            <ShardHeatmap shards={shards} mode={mode} selected={selected} onSelect={setSelected} nowMs={nowMs} />
          )}
          <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
            {mode === "owner" &&
              owners.map((o) => (
                <span key={o} className="inline-flex items-center gap-1.5">
                  <span className="h-2.5 w-2.5 rounded-sm" style={{ background: cellColor({ owner: o, tasks: 1 } as never, "owner", ownerIndex, nowMs) }} />
                  {o} <span className="opacity-60">(dim = no tasks)</span>
                </span>
              ))}
            {mode === "load" && <span>dark = idle · blue → red = more pending/running (log scale)</span>}
            {mode === "snapshot_age" && <span>green = fresh · red = 2 h+ · amber = never snapshotted · dark = no history</span>}
            {mode === "dirty" && <span>dark = covered · brighter = more records not yet in a snapshot</span>}
            <span className="inline-flex items-center gap-1.5">
              <span className="h-2.5 w-2.5 rounded-sm bg-[#7f1d1d]" /> unowned
            </span>
          </div>
        </div>

        <Card className="gap-0 py-0 self-start">
          <CardHeader className="flex flex-row items-center justify-between px-5 pt-5 pb-0">
            <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              {selected === null ? "Shard details" : `Shard ${selected}`}
            </CardTitle>
            {selected !== null && (
              <Button variant="ghost" size="icon" onClick={() => setSelected(null)} aria-label="Close">
                <X className="h-4 w-4" />
              </Button>
            )}
          </CardHeader>
          <CardContent className="px-5 pb-4 pt-2">
            {selected === null && <p className="py-6 text-center text-sm text-muted-foreground">Click a cell to inspect a shard.</p>}
            {selected !== null && !detail && <Skeleton className="h-40 w-full" />}
            {detail && (
              <>
                <Row label="Owner" value={detail.owner ?? <span className="text-red-400">none</span>} />
                <Row label="Epoch" value={detail.epoch} />
                <Row label="Tasks in RAM" value={detail.tasks} />
                <Row label="Pending / Running / Retry" value={`${detail.pending} / ${detail.running} / ${detail.retry}`} />
                <Row label="Signals · Dead letters" value={`${detail.signals} · ${detail.dead_letters}`} />
                <Row label="Shard seq" value={detail.shard_seq} />
                <Row label="Snapshot" value={detail.snapshot_lsn ? `${detail.snapshot_lsn} (seq ${detail.snapshot_seq})` : "never"} />
                <Row label="Snapshot taken" value={detail.snapshot_at ? formatRelative(detail.snapshot_at) : "—"} />
                <Row label="Records since snapshot" value={detail.records_since_snapshot} />
                <Row label="Dirty since LSN" value={detail.dirty_since_lsn ?? "—"} />
                {Object.keys(detail.queues).length > 0 && (
                  <div className="mt-3">
                    <p className="mb-1 text-xs font-medium uppercase tracking-wider text-muted-foreground">Queues</p>
                    {Object.entries(detail.queues).map(([q, c]) => (
                      <Row key={q} label={q} value={counts(c)} />
                    ))}
                  </div>
                )}
                {detail.snapshot_at && (
                  <p className="mt-3 text-xs text-muted-foreground">Snapshot at {formatDate(detail.snapshot_at)}</p>
                )}
              </>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
