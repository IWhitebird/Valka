import { Link } from "react-router-dom";
import { Database, HardDrive, Users } from "lucide-react";
import type { ClusterNode } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { formatDurationMs, nodeStatusDot } from "@/lib/cluster";
import { cn, formatRelative } from "@/lib/utils";

export function NodeCard({ node, numShards }: { node: ClusterNode; numShards: number }) {
  const poisoned = node.wal.poisoned !== null;
  return (
    <Link to={`/cluster/nodes/${encodeURIComponent(node.node_id)}`} className="block">
      <Card className={cn("gap-0 py-0 transition-colors hover:bg-accent/40", poisoned && "border-red-500/40")}>
        <CardContent className="p-5">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <span className={cn("h-2 w-2 rounded-full", nodeStatusDot(node.status))} />
                <span className="truncate font-medium text-foreground">{node.node_id}</span>
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                {node.status} · up {formatRelative(node.started_at).replace(" ago", "")} · v{node.version}
              </p>
            </div>
            <Badge variant="outline" className="shrink-0 font-mono">
              epoch {node.epoch}
            </Badge>
          </div>

          <dl className="mt-4 grid grid-cols-3 gap-3 text-sm">
            <div>
              <dt className="text-xs text-muted-foreground">Shards</dt>
              <dd className="font-medium">
                {node.shards_owned.toLocaleString()}
                <span className="text-muted-foreground">/{numShards.toLocaleString()}</span>
              </dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">Pending / Running</dt>
              <dd className="font-medium">
                {node.tasks.pending} / {node.tasks.running}
              </dd>
            </div>
            <div>
              <dt className="flex items-center gap-1 text-xs text-muted-foreground">
                <Users className="h-3 w-3" /> Workers
              </dt>
              <dd className="font-medium">{node.workers_connected}</dd>
            </div>
            <div>
              <dt className="flex items-center gap-1 text-xs text-muted-foreground">
                <Database className="h-3 w-3" /> Durable LSN
              </dt>
              <dd className="font-mono text-xs">{node.wal.durable_lsn}</dd>
            </div>
            <div>
              <dt className="text-xs text-muted-foreground">Unflushed</dt>
              <dd className={cn("font-medium", node.wal.unflushed_records > 0 && "text-amber-400")}>
                {node.wal.unflushed_records}
                {node.wal.oldest_unacked_ms !== null && (
                  <span className="ml-1 text-xs text-muted-foreground">
                    ({formatDurationMs(node.wal.oldest_unacked_ms)})
                  </span>
                )}
              </dd>
            </div>
            <div>
              <dt className="flex items-center gap-1 text-xs text-muted-foreground">
                <HardDrive className="h-3 w-3" /> Dirty shards
              </dt>
              <dd className="font-medium">{node.snapshots.dirty_shards}</dd>
            </div>
          </dl>

          {poisoned && (
            <p className="mt-3 rounded-md border border-red-500/30 bg-red-500/5 p-2 text-xs text-red-400">
              Writer poisoned: {node.wal.poisoned}
            </p>
          )}
        </CardContent>
      </Card>
    </Link>
  );
}
