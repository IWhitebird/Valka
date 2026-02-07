import { Activity } from "lucide-react";
import type { TaskRun } from "@/api/types";
import { cn, truncateId, formatDate } from "@/lib/utils";
import { TaskStatusBadge } from "@/components/tasks/task-status-badge";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";

interface TaskRunsTableProps {
  runs: TaskRun[];
  isLoading: boolean;
  selectedRunId: string | null;
  onSelectRun: (runId: string) => void;
}

function SkeletonRows() {
  return (
    <>
      {Array.from({ length: 3 }).map((_, i) => (
        <TableRow key={i}>
          <TableCell><Skeleton className="h-4 w-16" /></TableCell>
          <TableCell><Skeleton className="h-4 w-8" /></TableCell>
          <TableCell><Skeleton className="h-5 w-20 rounded-full" /></TableCell>
          <TableCell><Skeleton className="h-4 w-16" /></TableCell>
          <TableCell><Skeleton className="h-4 w-28" /></TableCell>
          <TableCell><Skeleton className="h-4 w-28" /></TableCell>
          <TableCell><Skeleton className="h-4 w-32" /></TableCell>
        </TableRow>
      ))}
    </>
  );
}

export function TaskRunsTable({
  runs,
  isLoading,
  selectedRunId,
  onSelectRun,
}: TaskRunsTableProps) {
  if (!isLoading && runs.length === 0) {
    return (
      <div className="flex h-32 flex-col items-center justify-center rounded-lg border bg-card">
        <Activity className="h-8 w-8 text-muted-foreground/50" />
        <p className="mt-2 text-sm text-muted-foreground">No runs yet</p>
      </div>
    );
  }

  return (
    <div className="overflow-hidden rounded-lg border">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Run ID
            </TableHead>
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Attempt
            </TableHead>
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Status
            </TableHead>
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Worker
            </TableHead>
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Started
            </TableHead>
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Completed
            </TableHead>
            <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
              Error
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {isLoading ? (
            <SkeletonRows />
          ) : (
            runs.map((run) => (
              <TableRow
                key={run.id}
                onClick={() => onSelectRun(run.id)}
                data-state={selectedRunId === run.id ? "selected" : undefined}
                className={cn(
                  "cursor-pointer",
                  selectedRunId === run.id && "bg-accent",
                )}
              >
                <TableCell className="px-4 font-mono text-xs text-muted-foreground">
                  {truncateId(run.id)}
                </TableCell>
                <TableCell className="px-4">#{run.attempt_number}</TableCell>
                <TableCell className="px-4">
                  <TaskStatusBadge status={run.status} />
                </TableCell>
                <TableCell className="px-4 font-mono text-xs text-muted-foreground">
                  {run.worker_id ? truncateId(run.worker_id) : "--"}
                </TableCell>
                <TableCell className="px-4 text-xs text-muted-foreground">
                  {formatDate(run.started_at)}
                </TableCell>
                <TableCell className="px-4 text-xs text-muted-foreground">
                  {run.completed_at ? formatDate(run.completed_at) : "--"}
                </TableCell>
                <TableCell className="max-w-[200px] truncate px-4 text-xs text-red-400">
                  {run.error_message || "--"}
                </TableCell>
              </TableRow>
            ))
          )}
        </TableBody>
      </Table>
    </div>
  );
}
