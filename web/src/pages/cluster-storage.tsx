import { RefreshCw } from "lucide-react";
import { ClusterTabs } from "@/components/cluster/cluster-tabs";
import { StorageCards } from "@/components/cluster/storage-cards";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { useClusterOverview, useStorage } from "@/hooks/use-cluster";
import { formatRelative } from "@/lib/utils";

export function ClusterStoragePage() {
  const { data: storage, isLoading, isError, error, refetch } = useStorage();
  const { data: overview } = useClusterOverview();

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">Storage</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            The bucket is the only durable state. Segments below every snapshot are truncated automatically.
          </p>
        </div>
        <Button variant="outline" size="icon" onClick={() => refetch()}>
          <RefreshCw className="h-4 w-4" />
        </Button>
      </div>
      <ClusterTabs />

      {isLoading && <Skeleton className="h-28 w-full" />}
      {isError && <p className="text-sm text-red-400">Could not load storage stats: {(error as Error).message}</p>}
      {storage && (
        <>
          <StorageCards storage={storage} />
          <Card className="gap-0 py-0">
            <CardHeader className="px-5 pt-5 pb-0">
              <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                Layout
              </CardTitle>
            </CardHeader>
            <CardContent className="px-5 pb-5 pt-2">
              <pre className="overflow-x-auto rounded-md bg-muted/40 p-3 font-mono text-xs leading-relaxed text-muted-foreground">
{`backend            ${storage.backend}
assignment         shard → {node, epoch}, compare-and-swap
wal/{node}/        ${storage.wal.segments} segment(s) — group-committed records, header + CRC-framed zstd JSON
snapshots/{shard}/ ${storage.snapshots.count} snapshot(s) — per-shard state, replay starts here
logs/{run}/        ${storage.logs.chunks} chunk(s) — task log lines`}
              </pre>
              <p className="mt-3 text-xs text-muted-foreground">
                Computed {formatRelative(storage.computed_at)} (cached 30 s).
                {overview && ` Nodes: ${overview.nodes.map((n) => n.node_id).join(", ")}.`}
                {" "}The cost figure is an upper bound: one PUT per flush window while busy, at S3 Standard pricing.
              </p>
            </CardContent>
          </Card>
        </>
      )}
    </div>
  );
}
