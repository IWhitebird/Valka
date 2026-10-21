import { useState } from "react";
import { Link } from "react-router-dom";
import { KeyRound, Pin, PinOff, Power, SlidersHorizontal } from "lucide-react";
import { adminApi } from "@/api/admin";
import { type ClusterNode, hasStats } from "@/api/types";
import { ClusterTabs } from "@/components/cluster/cluster-tabs";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useAdminAction, useAudit, useQueues } from "@/hooks/use-admin";
import { useClusterOverview, usePlacement } from "@/hooks/use-cluster";
import { credentials } from "@/lib/credentials";
import { nodeStatusDot } from "@/lib/cluster";
import { cn, formatRelative } from "@/lib/utils";

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
      {children}
    </CardTitle>
  );
}

function Outcome({ ok, error }: { ok?: string | null; error?: string | null }) {
  if (!ok && !error) return null;
  return (
    <p
      role="status"
      aria-live="polite"
      className={cn(
        "mt-3 rounded-md border px-3 py-2 text-sm",
        error
          ? "border-destructive/30 bg-destructive/10 text-destructive"
          : "border-primary/30 bg-primary/10 text-foreground",
      )}
    >
      {error ?? ok}
    </p>
  );
}

function AdminToken({ onChange }: { onChange: () => void }) {
  const [token, setToken] = useState("");
  const current = credentials.adminToken();
  if (current) {
    return (
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-card px-4 py-3">
        <span className="flex items-center gap-2 text-sm text-foreground">
          <KeyRound className="h-4 w-4 text-primary" /> Admin token set for this browser tab.
        </span>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            credentials.setAdminToken(null);
            onChange();
          }}
        >
          Forget token
        </Button>
      </div>
    );
  }
  return (
    <form
      className="rounded-lg border bg-card p-4"
      onSubmit={(e) => {
        e.preventDefault();
        credentials.setAdminToken(token.trim());
        setToken("");
        onChange();
      }}
    >
      <div className="flex flex-wrap items-end gap-3">
        <div className="min-w-[260px] flex-1 space-y-1.5">
          <Label htmlFor="admin-token">Admin token</Label>
          <Input
            id="admin-token"
            type="password"
            autoComplete="off"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="The server's VALKA_ADMIN_TOKEN"
          />
        </div>
        <Button type="submit" disabled={token.trim() === ""}>
          Use token
        </Button>
      </div>
      <p className="mt-2 text-xs text-muted-foreground">
        Draining nodes, moving shards and changing partitions need it. It is kept for this tab
        only, and every action is recorded in the audit log below.
      </p>
    </form>
  );
}

function DrainDialog({
  node,
  onClose,
}: {
  node: ClusterNode | null;
  onClose: (message?: string) => void;
}) {
  const drain = useAdminAction((n: string) => adminApi.drainNode(n));
  const shards = node && hasStats(node) ? node.shards_owned : null;
  return (
    <Dialog open={node !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Drain {node?.node_id}?</DialogTitle>
          <DialogDescription>
            It hands {shards === null ? "its" : `its ${shards.toLocaleString()}`} shards to the
            other nodes with final snapshots, so nothing has to be replayed, then marks itself
            dead and stops. Its workers reconnect to other nodes. Start the process again to
            bring it back.
          </DialogDescription>
        </DialogHeader>
        {drain.isError && <Outcome error={(drain.error as Error).message} />}
        <DialogFooter>
          <Button variant="outline" onClick={() => onClose()}>
            Keep it running
          </Button>
          <Button
            variant="destructive"
            disabled={drain.isPending || !node}
            onClick={() =>
              node &&
              drain.mutate(node.node_id, {
                onSuccess: () => onClose(`${node.node_id} is handing its shards over.`),
              })
            }
          >
            <Power className="h-4 w-4" />
            {drain.isPending ? "Draining…" : "Drain node"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Nodes({ canAct }: { canAct: boolean }) {
  const { data: overview } = useClusterOverview();
  const [draining, setDraining] = useState<ClusterNode | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const nodes = overview?.nodes ?? [];
  return (
    <Card className="gap-0 py-0">
      <CardHeader className="px-5 pt-5 pb-0">
        <SectionTitle>Nodes</SectionTitle>
      </CardHeader>
      <CardContent className="px-5 pb-4 pt-2">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Node</TableHead>
              <TableHead>Incarnation</TableHead>
              <TableHead className="text-right">Shards</TableHead>
              <TableHead className="text-right">Workers</TableHead>
              <TableHead className="w-0" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {nodes.map((n) => {
              const live = n.status === "alive";
              return (
                <TableRow key={n.node_id}>
                  <TableCell>
                    <Link
                      to={`/cluster/nodes/${encodeURIComponent(n.node_id)}`}
                      className="inline-flex items-center gap-2 font-medium hover:underline"
                    >
                      <span className={cn("h-2 w-2 rounded-full", nodeStatusDot(n.status))} />
                      {n.node_id}
                    </Link>
                    <span className="ml-2 text-xs text-muted-foreground">{n.status}</span>
                  </TableCell>
                  <TableCell className="font-mono text-sm">{n.inc}</TableCell>
                  <TableCell className="text-right font-mono text-sm tabular-nums">
                    {hasStats(n) ? n.shards_owned.toLocaleString() : "—"}
                  </TableCell>
                  <TableCell className="text-right font-mono text-sm tabular-nums">
                    {hasStats(n) ? n.workers_connected : "—"}
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!canAct || !live}
                      title={!canAct ? "Set the admin token first" : !live ? "Only a live node can drain" : undefined}
                      onClick={() => setDraining(n)}
                    >
                      <Power className="h-3.5 w-3.5" /> Drain
                    </Button>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
        <Outcome ok={message} />
      </CardContent>
      <DrainDialog
        node={draining}
        onClose={(msg) => {
          setDraining(null);
          if (msg) setMessage(msg);
        }}
      />
    </Card>
  );
}

function Placement({ canAct }: { canAct: boolean }) {
  const { data: overview } = useClusterOverview();
  const { data: placement } = usePlacement();
  const [shard, setShard] = useState("");
  const [to, setTo] = useState("");
  const move = useAdminAction(({ shard, to }: { shard: number; to: string }) =>
    adminApi.moveShard(shard, to),
  );
  const unpin = useAdminAction((s: number) => adminApi.unpinShard(s));
  const [message, setMessage] = useState<string | null>(null);
  const numShards = overview?.num_shards ?? 0;
  const shardNumber = Number(shard);
  const validShard = shard !== "" && Number.isInteger(shardNumber) && shardNumber >= 0 && shardNumber < numShards;
  const pins = Object.entries(placement?.pins ?? {}).sort((a, b) => Number(a[0]) - Number(b[0]));
  const error = (move.error ?? unpin.error) as Error | null;

  return (
    <Card className="gap-0 py-0">
      <CardHeader className="px-5 pt-5 pb-0">
        <SectionTitle>Shard placement</SectionTitle>
      </CardHeader>
      <CardContent className="space-y-4 px-5 pb-5 pt-2">
        <p className="text-sm text-muted-foreground">
          Shards go where rendezvous hashing puts them. A pin overrides that while its node is
          eligible; the shard moves on the next placement round.
        </p>
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            move.mutate(
              { shard: shardNumber, to },
              {
                onSuccess: () => {
                  setMessage(`Shard ${shardNumber} is pinned to ${to}.`);
                  setShard("");
                },
              },
            );
          }}
        >
          <div className="w-32 space-y-1.5">
            <Label htmlFor="move-shard">Shard</Label>
            <Input
              id="move-shard"
              inputMode="numeric"
              placeholder={numShards ? `0–${numShards - 1}` : "0"}
              value={shard}
              onChange={(e) => setShard(e.target.value.trim())}
            />
          </div>
          <div className="w-48 space-y-1.5">
            <Label htmlFor="move-to">Move to</Label>
            <Select value={to} onValueChange={setTo}>
              <SelectTrigger id="move-to">
                <SelectValue placeholder="Eligible node" />
              </SelectTrigger>
              <SelectContent>
                {(placement?.eligible_nodes ?? []).map((n) => (
                  <SelectItem key={n} value={n}>
                    {n}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button type="submit" disabled={!canAct || !validShard || !to || move.isPending}>
            <Pin className="h-4 w-4" /> {move.isPending ? "Pinning…" : "Move and pin"}
          </Button>
        </form>
        {pins.length > 0 ? (
          <div className="flex flex-wrap gap-2">
            {pins.map(([s, node]) => (
              <Badge key={s} variant="outline" className="gap-1.5 py-1 pl-2.5 pr-1 font-mono">
                {s} → {node}
                <button
                  type="button"
                  className="rounded p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-40"
                  disabled={!canAct || unpin.isPending}
                  aria-label={`Unpin shard ${s}`}
                  onClick={() =>
                    unpin.mutate(Number(s), {
                      onSuccess: () => setMessage(`Shard ${s} is back under automatic placement.`),
                    })
                  }
                >
                  <PinOff className="h-3.5 w-3.5" />
                </button>
              </Badge>
            ))}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">No pins: every shard is placed automatically.</p>
        )}
        <Outcome ok={error ? null : message} error={error?.message} />
      </CardContent>
    </Card>
  );
}

function PartitionsDialog({
  queue,
  current,
  onClose,
}: {
  queue: string | null;
  current: number;
  onClose: (message?: string) => void;
}) {
  const [value, setValue] = useState("");
  const set = useAdminAction(({ queue, n }: { queue: string; n: number }) =>
    adminApi.setPartitions(queue, n),
  );
  const n = Number(value);
  const valid = Number.isInteger(n) && n >= 1 && n <= 4096;
  return (
    <Dialog
      open={queue !== null}
      onOpenChange={(open) => {
        if (!open) {
          setValue("");
          onClose();
        }
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Partitions of {queue}</DialogTitle>
          <DialogDescription>
            Now {current}. More partitions spread the queue over more shards and nodes; new
            ones take new tasks at once. Fewer: the removed partitions take no new tasks and
            are served until empty. Idempotency keys stay exact either way.
          </DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (!queue) return;
            set.mutate(
              { queue, n },
              {
                onSuccess: (r) => {
                  setValue("");
                  onClose(`${r.queue} now has ${r.partitions} partitions (was ${r.previous ?? current}).`);
                },
              },
            );
          }}
        >
          <div className="space-y-1.5">
            <Label htmlFor="partitions">Partitions (1–4096)</Label>
            <Input
              id="partitions"
              inputMode="numeric"
              autoFocus
              value={value}
              placeholder={String(current)}
              onChange={(e) => setValue(e.target.value.trim())}
            />
          </div>
          {set.isError && <Outcome error={(set.error as Error).message} />}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onClose()}>
              Cancel
            </Button>
            <Button type="submit" disabled={!valid || n === current || set.isPending}>
              <SlidersHorizontal className="h-4 w-4" />
              {set.isPending ? "Saving…" : "Set partitions"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function Queues({ canAct }: { canAct: boolean }) {
  const { data: queues = [], isLoading } = useQueues();
  const [editing, setEditing] = useState<{ queue: string; current: number } | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  return (
    <Card className="gap-0 py-0">
      <CardHeader className="px-5 pt-5 pb-0">
        <SectionTitle>Queues</SectionTitle>
      </CardHeader>
      <CardContent className="px-5 pb-4 pt-2">
        {!isLoading && queues.length === 0 ? (
          <p className="py-4 text-sm text-muted-foreground">
            No queues yet. A queue appears with its first task, with 16 partitions.
          </p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Queue</TableHead>
                <TableHead className="text-right">Partitions</TableHead>
                <TableHead className="text-right">Draining</TableHead>
                <TableHead className="w-0" />
              </TableRow>
            </TableHeader>
            <TableBody>
              {queues.map((q) => (
                <TableRow key={q.queue}>
                  <TableCell className="font-medium">{q.queue}</TableCell>
                  <TableCell className="text-right font-mono text-sm tabular-nums">{q.partitions}</TableCell>
                  <TableCell className="text-right font-mono text-sm tabular-nums">
                    {q.served_partitions > q.partitions ? q.served_partitions - q.partitions : "—"}
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={!canAct}
                      onClick={() => setEditing({ queue: q.queue, current: q.partitions })}
                    >
                      Change
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
        <Outcome ok={message} />
      </CardContent>
      <PartitionsDialog
        queue={editing?.queue ?? null}
        current={editing?.current ?? 0}
        onClose={(msg) => {
          setEditing(null);
          if (msg) setMessage(msg);
        }}
      />
    </Card>
  );
}

function Audit({ canAct }: { canAct: boolean }) {
  const { data: records = [], isError, error } = useAudit(canAct);
  return (
    <Card className="gap-0 py-0">
      <CardHeader className="px-5 pt-5 pb-0">
        <SectionTitle>Audit log</SectionTitle>
      </CardHeader>
      <CardContent className="px-5 pb-4 pt-2">
        {!canAct ? (
          <p className="py-4 text-sm text-muted-foreground">Set the admin token to see who changed what.</p>
        ) : isError ? (
          <Outcome error={(error as Error).message} />
        ) : records.length === 0 ? (
          <p className="py-4 text-sm text-muted-foreground">No admin actions yet.</p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>When</TableHead>
                <TableHead>Action</TableHead>
                <TableHead>Details</TableHead>
                <TableHead>Through</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {records.map((r) => (
                <TableRow key={r.id}>
                  <TableCell className="whitespace-nowrap text-sm text-muted-foreground" title={r.at}>
                    {formatRelative(r.at)}
                  </TableCell>
                  <TableCell className="font-mono text-sm">{r.action}</TableCell>
                  <TableCell className="max-w-md truncate font-mono text-xs text-muted-foreground">
                    {JSON.stringify(r.params)}
                  </TableCell>
                  <TableCell className="text-sm">{r.node}</TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  );
}

export function ClusterOperationsPage() {
  const [, refresh] = useState(0);
  const canAct = credentials.adminToken() !== null;
  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">Operations</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Drain nodes, move shards and resize queues. These change where data lives, so they need
          the admin token.
        </p>
      </div>
      <ClusterTabs />
      <AdminToken onChange={() => refresh((n) => n + 1)} />
      <div className="grid gap-6 xl:grid-cols-2">
        <Nodes canAct={canAct} />
        <Placement canAct={canAct} />
        <Queues canAct={canAct} />
        <Audit canAct={canAct} />
      </div>
    </div>
  );
}
