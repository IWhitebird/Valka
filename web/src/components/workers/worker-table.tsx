import type { Worker } from "@/api/types";
import { cn, formatRelative, truncateId } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";

interface WorkerTableProps {
  workers: Worker[];
  isLoading: boolean;
}

function workerStatusDot(status: string): string {
  switch (status.toUpperCase()) {
    case "ACTIVE":
    case "CONNECTED":
      return "bg-emerald-400";
    case "IDLE":
      return "bg-zinc-400";
    case "DRAINING":
      return "bg-amber-400";
    case "DISCONNECTED":
      return "bg-red-400";
    default:
      return "bg-zinc-400";
  }
}

function LoadingSkeleton() {
  return (
    <div className="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Worker
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Status
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Queues
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Concurrency
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Active
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Last Heartbeat
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Connected
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {Array.from({ length: 4 }).map((_, i) => (
            <TableRow key={i} className="hover:bg-transparent">
              <TableCell>
                <div className="space-y-1.5">
                  <Skeleton className="h-4 w-28" />
                  <Skeleton className="h-3 w-16" />
                </div>
              </TableCell>
              <TableCell>
                <Skeleton className="h-4 w-20" />
              </TableCell>
              <TableCell>
                <div className="flex gap-1">
                  <Skeleton className="h-5 w-14 rounded-full" />
                  <Skeleton className="h-5 w-12 rounded-full" />
                </div>
              </TableCell>
              <TableCell>
                <Skeleton className="h-4 w-8" />
              </TableCell>
              <TableCell>
                <Skeleton className="h-4 w-12" />
              </TableCell>
              <TableCell>
                <Skeleton className="h-3 w-24" />
              </TableCell>
              <TableCell>
                <Skeleton className="h-3 w-24" />
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

export function WorkerTable({ workers, isLoading }: WorkerTableProps) {
  if (isLoading) {
    return <LoadingSkeleton />;
  }

  if (workers.length === 0) {
    return (
      <div className="flex h-64 items-center justify-center rounded-lg border">
        <div className="text-center">
          <p className="text-sm text-muted-foreground">No workers connected</p>
          <p className="mt-1 text-xs text-muted-foreground/60">
            Workers will appear here when they connect to the server
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Worker
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Status
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Queues
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Concurrency
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Active
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Last Heartbeat
            </TableHead>
            <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              Connected
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {workers.map((worker) => (
            <TableRow key={worker.id}>
              <TableCell>
                <div>
                  <p className="text-sm font-medium text-foreground">{worker.name}</p>
                  <p className="font-mono text-xs text-muted-foreground">
                    {truncateId(worker.id)}
                  </p>
                </div>
              </TableCell>
              <TableCell>
                <span className="inline-flex items-center gap-2 text-sm">
                  <span
                    className={cn(
                      "h-2 w-2 shrink-0 rounded-full",
                      workerStatusDot(worker.status),
                    )}
                  />
                  <span className="text-foreground">{worker.status}</span>
                </span>
              </TableCell>
              <TableCell>
                <div className="flex flex-wrap gap-1">
                  {worker.queues.map((queue) => (
                    <Badge key={queue} variant="secondary" className="text-xs font-normal">
                      {queue}
                    </Badge>
                  ))}
                </div>
              </TableCell>
              <TableCell className="text-foreground">{worker.concurrency}</TableCell>
              <TableCell>
                <span className="text-foreground">{worker.active_tasks}</span>
                <span className="text-muted-foreground/60"> / {worker.concurrency}</span>
              </TableCell>
              <TableCell className="text-xs text-muted-foreground">
                {formatRelative(worker.last_heartbeat)}
              </TableCell>
              <TableCell className="text-xs text-muted-foreground">
                {formatRelative(worker.connected_at)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
