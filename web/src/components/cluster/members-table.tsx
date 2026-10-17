import { Link } from "react-router-dom";
import type { ClusterMember, ClusterPlacement } from "@/api/types";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { formatDurationMs, nodeStatusDot } from "@/lib/cluster";
import { cn } from "@/lib/utils";

/**
 * Membership as the serving node sees it: one row per lease in the bucket. Nodes never ask
 * each other who is alive; a lease whose ETag stops changing for the lease timeout (by this
 * node's own clock) is suspect, and its guardian tombstones it.
 */
export function MembersTable({
  members,
  placement,
  thisNode,
}: {
  members: ClusterMember[];
  placement?: ClusterPlacement;
  thisNode: string;
}) {
  const pins = placement ? Object.entries(placement.pins) : [];
  return (
    <Card className="gap-0 py-0">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 px-5 pt-5 pb-0">
        <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          Membership (leases in the bucket, seen from {thisNode})
        </CardTitle>
        {placement && (
          <span className="text-xs text-muted-foreground">
            effective protocol {placement.effective_protocol} · {placement.eligible_nodes.length} eligible
          </span>
        )}
      </CardHeader>
      <CardContent className="px-5 pb-4 pt-2">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Node</TableHead>
              <TableHead>Incarnation</TableHead>
              <TableHead>Lease</TableHead>
              <TableHead>Lease age</TableHead>
              <TableHead>Placement</TableHead>
              <TableHead>Protocol</TableHead>
              <TableHead>Unsplit logs</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {members.map((m) => (
              <TableRow key={m.node_id}>
                <TableCell>
                  <Link
                    to={`/cluster/nodes/${encodeURIComponent(m.node_id)}`}
                    className="inline-flex items-center gap-2 font-medium hover:underline"
                  >
                    <span className={cn("h-2 w-2 rounded-full", nodeStatusDot(m.status))} />
                    {m.node_id}
                  </Link>
                </TableCell>
                <TableCell className="font-mono text-sm">{m.inc}</TableCell>
                <TableCell>
                  <Badge variant="outline">{m.status}</Badge>
                </TableCell>
                <TableCell
                  className={cn(
                    "font-mono text-sm",
                    m.status !== "dead" && m.lease_age_ms > 7_000 && "text-amber-400",
                  )}
                >
                  {formatDurationMs(m.lease_age_ms)}
                </TableCell>
                <TableCell className="text-sm">
                  {m.eligible ? "eligible" : <span className="text-muted-foreground">not eligible</span>}
                </TableCell>
                <TableCell className="font-mono text-sm">{m.protocol}</TableCell>
                <TableCell className="font-mono text-sm">
                  {m.unsplit.length === 0 ? "—" : m.unsplit.join(", ")}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
        {pins.length > 0 && (
          <p className="mt-3 text-xs text-muted-foreground">
            Operator pins:{" "}
            {pins.map(([shard, node]) => (
              <span key={shard} className="mr-2 font-mono">
                {shard}→{node}
              </span>
            ))}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
