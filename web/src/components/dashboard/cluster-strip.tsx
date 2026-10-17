import { Link } from "react-router-dom";
import { Server } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { useClusterOverview } from "@/hooks/use-cluster";
import { healthTone, sumStats } from "@/lib/cluster";
import { cn } from "@/lib/utils";

export function ClusterStrip() {
  const { data } = useClusterOverview();
  if (!data) return null;
  const tone = healthTone(data.health.status);
  const alive = data.nodes.filter((n) => n.status === "alive").length;
  const owned = sumStats(data.nodes, (n) => n.shards_owned);
  const unflushed = sumStats(data.nodes, (n) => n.wal.unflushed_records);
  const dirty = sumStats(data.nodes, (n) => n.snapshots.dirty_shards);
  return (
    <Link
      to="/cluster"
      className={cn("flex flex-wrap items-center gap-x-6 gap-y-2 rounded-lg border px-4 py-3 text-sm transition-colors hover:bg-accent/40", tone.bar)}
    >
      <span className="flex items-center gap-2 font-medium text-foreground">
        <Server className="h-4 w-4" /> Cluster
        <Badge variant="outline" className={tone.badge}>{tone.label}</Badge>
      </span>
      <span className="text-muted-foreground">
        <span className="text-foreground">{alive}/{data.nodes.length}</span> nodes alive
      </span>
      <span className="text-muted-foreground">
        <span className="text-foreground">{owned.toLocaleString()}/{data.num_shards.toLocaleString()}</span> shards owned
      </span>
      <span className="text-muted-foreground">
        <span className={cn("text-foreground", unflushed > 0 && "text-amber-400")}>{unflushed}</span> unflushed records
      </span>
      <span className="text-muted-foreground">
        <span className="text-foreground">{dirty}</span> dirty shards
      </span>
    </Link>
  );
}
