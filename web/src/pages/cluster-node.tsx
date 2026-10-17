import { Link, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft } from "lucide-react";
import { workersApi } from "@/api/workers";
import { ClusterTabs } from "@/components/cluster/cluster-tabs";
import { SnapshotPanel } from "@/components/cluster/snapshot-panel";
import { Stat } from "@/components/cluster/stat";
import { WalPanel } from "@/components/cluster/wal-panel";
import { hasStats } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { WorkerTable } from "@/components/workers/worker-table";
import { useClusterOverview, useMembers, useShards } from "@/hooks/use-cluster";
import { compressRanges, formatDurationMs, nodeStatusDot } from "@/lib/cluster";
import { cn, formatDate } from "@/lib/utils";

export function ClusterNodePage() {
  const { nodeId = "" } = useParams<{ nodeId: string }>();
  const { data: overview, isLoading } = useClusterOverview();
  const { data: shards = [] } = useShards({ node: nodeId });
  const { data: workers = [], isLoading: workersLoading } = useQuery({
    queryKey: ["workers"],
    queryFn: workersApi.list,
    refetchInterval: 5_000,
  });
  const { data: members = [] } = useMembers();
  const node = overview?.nodes.find((n) => n.node_id === nodeId);
  const member = members.find((m) => m.node_id === nodeId);
  const stats = node && hasStats(node) ? node : null;
  const nodeWorkers = workers.filter((w) => !w.node_id || w.node_id === nodeId);

  return (
    <div className="space-y-6">
      <Link to="/cluster" className="inline-flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground">
        <ArrowLeft className="h-4 w-4" /> Back to cluster
      </Link>
      <ClusterTabs />

      {isLoading && <Skeleton className="h-24 w-full" />}
      {overview && !node && (
        <p className="text-sm text-muted-foreground">Node "{nodeId}" is not part of this cluster.</p>
      )}
      {overview && node && (
        <>
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="flex items-center gap-2">
                <span className={cn("h-2.5 w-2.5 rounded-full", nodeStatusDot(node.status))} />
                <h1 className="text-2xl font-semibold tracking-tight text-foreground">{node.node_id}</h1>
                <Badge variant="outline" className="font-mono" title="Incarnation: bumped on every start">
                  inc {node.inc}
                </Badge>
                <Badge variant="outline">{node.status}</Badge>
                {node.eligible && <Badge variant="outline">eligible for placement</Badge>}
              </div>
              <p className="mt-1 font-mono text-xs text-muted-foreground">
                gRPC {node.grpc_addr ?? "—"} · HTTP {node.http_addr ?? "—"}
                {member?.internal_addr && ` · internal ${member.internal_addr}`}
                {node.version && ` · v${node.version}`}
                {stats && ` · ${stats.storage_backend}`}
                {member && ` · protocol ${member.protocol}`}
                {node.started_at && ` · started ${formatDate(node.started_at)}`}
              </p>
              {node.lease_age_ms !== null && (
                <p className="mt-1 text-xs text-muted-foreground">
                  Lease last changed {formatDurationMs(node.lease_age_ms)} ago
                  {member && member.unsplit.length > 0 && ` · earlier incarnations not split yet: ${member.unsplit.join(", ")}`}
                </p>
              )}
            </div>
          </div>

          {!stats && (
            <p className="rounded-lg border p-4 text-sm text-muted-foreground">
              {node.status === "dead"
                ? "This node is gone: its lease was tombstoned or it exited. Its log was split and its shards were claimed by live nodes."
                : "This node is not answering. It is known only from its lease in the bucket; if its lease stops changing for the lease timeout, a live node declares it dead and takes its shards over."}
            </p>
          )}

          {stats && (
            <>
              <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
                <Stat label="Shards owned" value={`${stats.shards_owned.toLocaleString()} / ${overview.num_shards.toLocaleString()}`} hint={`${stats.shards_with_tasks} hold tasks`} />
                <Stat label="Pending / Running" value={`${stats.tasks.pending} / ${stats.tasks.running}`} hint={`${stats.tasks.retry} awaiting retry · ${stats.tasks.total} in RAM`} />
                <Stat label="Workers" value={stats.workers_connected} hint={`${stats.queues.length} queue${stats.queues.length === 1 ? "" : "s"}`} />
                <Stat
                  label="Unflushed records"
                  value={stats.wal.unflushed_records}
                  hint="appended, not yet durable"
                  tone={stats.wal.poisoned ? "bad" : stats.wal.unflushed_records > 0 ? "warn" : "default"}
                />
              </div>

              <div className="grid gap-6 lg:grid-cols-2">
                <WalPanel node={stats} />
                <SnapshotPanel
                  node={stats}
                  numShards={overview.num_shards}
                  local={stats.node_id === overview.this_node}
                />
              </div>
            </>
          )}

          <Card className="gap-0 py-0">
            <CardHeader className="px-5 pt-5 pb-0">
              <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                Shards owned
              </CardTitle>
            </CardHeader>
            <CardContent className="px-5 pb-5 pt-2">
              <p className="font-mono text-sm">{compressRanges(shards.map((s) => s.shard))}</p>
              <Link to={`/cluster/shards?node=${encodeURIComponent(node.node_id)}`} className="mt-2 inline-block text-xs text-primary hover:underline">
                Open in shard map →
              </Link>
            </CardContent>
          </Card>

          <div>
            <h2 className="mb-3 text-sm font-semibold text-foreground">Workers on this node</h2>
            <WorkerTable workers={nodeWorkers} isLoading={workersLoading} />
          </div>
        </>
      )}
    </div>
  );
}
