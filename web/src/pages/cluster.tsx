import { RefreshCw } from "lucide-react";
import { ClusterTabs } from "@/components/cluster/cluster-tabs";
import { HealthBanner } from "@/components/cluster/health-banner";
import { NodeCard } from "@/components/cluster/node-card";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { useClusterOverview } from "@/hooks/use-cluster";

export function ClusterPage() {
  const { data, isLoading, isError, error, refetch } = useClusterOverview();

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">Cluster</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Nodes, shard ownership and write-ahead log health
          </p>
        </div>
        <Button variant="outline" size="icon" onClick={() => refetch()}>
          <RefreshCw className="h-4 w-4" />
        </Button>
      </div>

      <ClusterTabs />

      {isLoading && <Skeleton className="h-20 w-full" />}
      {isError && (
        <p className="rounded-lg border border-red-500/30 bg-red-500/5 p-4 text-sm text-red-400">
          Could not load cluster state: {(error as Error).message}
        </p>
      )}
      {data && (
        <>
          <HealthBanner overview={data} />
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {data.nodes.map((n) => (
              <NodeCard key={n.node_id} node={n} numShards={data.num_shards} />
            ))}
          </div>
        </>
      )}
    </div>
  );
}
