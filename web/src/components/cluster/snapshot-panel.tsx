import { Camera } from "lucide-react";
import type { ClusterNode } from "@/api/types";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useSnapshotNow } from "@/hooks/use-cluster";
import { formatRelative } from "@/lib/utils";

function Row({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between border-b py-2.5 last:border-b-0">
      <span className="text-sm text-muted-foreground">{label}</span>
      <span className="font-mono text-sm">{value}</span>
    </div>
  );
}

export function SnapshotPanel({ node, numShards }: { node: ClusterNode; numShards: number }) {
  const s = node.snapshots;
  const snapshot = useSnapshotNow();
  return (
    <Card className="gap-0 py-0">
      <CardHeader className="flex flex-row items-center justify-between px-5 pt-5 pb-0">
        <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          Snapshots
        </CardTitle>
        <Button size="sm" variant="outline" onClick={() => snapshot.mutate()} disabled={snapshot.isPending}>
          <Camera className="mr-1.5 h-3.5 w-3.5" />
          {snapshot.isPending ? "Snapshotting…" : "Snapshot now"}
        </Button>
      </CardHeader>
      <CardContent className="px-5 pb-3 pt-2">
        <Row label="Last round" value={s.last_round_at ? formatRelative(s.last_round_at) : "not yet this run"} />
        <Row label="Dirty shards" value={`${s.dirty_shards} / ${numShards}`} />
        <Row label="Oldest uncovered LSN" value={s.oldest_dirty_lsn ?? "—"} />
        <Row label="Shards with a snapshot" value={s.shards_with_snapshot} />
        {snapshot.isError && (
          <p className="mt-2 text-xs text-red-400">Snapshot failed: {(snapshot.error as Error).message}</p>
        )}
      </CardContent>
    </Card>
  );
}
