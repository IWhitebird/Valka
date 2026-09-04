import type { ClusterNode } from "@/api/types";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { formatDurationMs } from "@/lib/cluster";
import { cn } from "@/lib/utils";

function Row({ label, value, tone }: { label: string; value: React.ReactNode; tone?: "warn" | "bad" }) {
  return (
    <div className="flex items-center justify-between border-b py-2.5 last:border-b-0">
      <span className="text-sm text-muted-foreground">{label}</span>
      <span className={cn("font-mono text-sm", tone === "warn" && "text-amber-400", tone === "bad" && "text-red-400")}>
        {value}
      </span>
    </div>
  );
}

export function WalPanel({ node }: { node: ClusterNode }) {
  const w = node.wal;
  return (
    <Card className="gap-0 py-0">
      <CardHeader className="px-5 pt-5 pb-0">
        <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          Write-ahead log
        </CardTitle>
      </CardHeader>
      <CardContent className="px-5 pb-3 pt-2">
        <Row label="Epoch" value={w.epoch} />
        <Row label="Durable LSN" value={w.durable_lsn} />
        <Row label="Next LSN" value={w.next_lsn} />
        <Row label="Unflushed records" value={w.unflushed_records} tone={w.unflushed_records > 0 ? "warn" : undefined} />
        <Row
          label="Oldest unacknowledged write"
          value={formatDurationMs(w.oldest_unacked_ms)}
          tone={w.oldest_unacked_ms !== null && w.oldest_unacked_ms > 2000 ? "warn" : undefined}
        />
        <Row label="Writer" value={w.poisoned ? `poisoned: ${w.poisoned}` : "healthy"} tone={w.poisoned ? "bad" : undefined} />
      </CardContent>
    </Card>
  );
}
