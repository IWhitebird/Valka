import { ShieldAlert, ShieldCheck } from "lucide-react";
import type { ClusterOverview } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { healthProblems, healthTone } from "@/lib/cluster";
import { cn } from "@/lib/utils";

export function HealthBanner({ overview }: { overview: ClusterOverview }) {
  const tone = healthTone(overview.health.status);
  const problems = healthProblems(overview.health, overview.num_shards);
  const alive = overview.nodes.filter((n) => n.status === "alive").length;
  const owned = overview.nodes.reduce((s, n) => s + n.shards_owned, 0);
  const unflushed = overview.nodes.reduce((s, n) => s + n.wal.unflushed_records, 0);
  const Icon = problems.length === 0 ? ShieldCheck : ShieldAlert;

  return (
    <div className={cn("flex items-start gap-4 rounded-lg border p-4", tone.bar)}>
      <Icon className="mt-0.5 h-5 w-5 shrink-0" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline" className={tone.badge}>
            {tone.label}
          </Badge>
          <span className="text-sm text-foreground">
            {alive}/{overview.nodes.length} node{overview.nodes.length === 1 ? "" : "s"} alive ·{" "}
            {owned.toLocaleString()}/{overview.num_shards.toLocaleString()} shards owned ·{" "}
            {unflushed === 0 ? "WAL current" : `${unflushed.toLocaleString()} records awaiting commit`}
          </span>
        </div>
        {problems.length > 0 && (
          <ul className="mt-2 space-y-1 text-sm text-muted-foreground">
            {problems.map((p) => (
              <li key={p}>• {p}</li>
            ))}
          </ul>
        )}
        {!overview.clustered && (
          <p className="mt-2 text-xs text-muted-foreground">
            Single-node mode: this node owns every shard. Multi-node ownership arrives with
            the cluster phase.
          </p>
        )}
      </div>
    </div>
  );
}
