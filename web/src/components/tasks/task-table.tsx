import { Link, useNavigate } from "react-router-dom";
import { ChevronLeft, ChevronRight, ListTodo, Trash2 } from "lucide-react";
import type { Task } from "@/api/types";
import { errorMessage } from "@/api/client";
import { truncateId, formatRelative } from "@/lib/utils";
import { useDeleteTask } from "@/hooks/use-tasks";
import { TaskStatusBadge } from "./task-status-badge";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import { ErrorNote } from "@/components/ui/error-note";
import { Skeleton } from "@/components/ui/skeleton";

interface TaskTableProps {
  tasks: Task[];
  isLoading: boolean;
  offset: number;
  limit: number;
  /** Whether a task exists after this page (the page asked the server for one more). */
  hasMore: boolean;
  onPageChange: (offset: number) => void;
}

function SkeletonRows() {
  return (
    <>
      {Array.from({ length: 6 }).map((_, i) => (
        <TableRow key={i}>
          <TableCell>
            <Skeleton className="h-4 w-16" />
          </TableCell>
          <TableCell>
            <Skeleton className="h-4 w-24" />
          </TableCell>
          <TableCell>
            <Skeleton className="h-4 w-28" />
          </TableCell>
          <TableCell>
            <Skeleton className="h-5 w-20 rounded-full" />
          </TableCell>
          <TableCell>
            <Skeleton className="h-4 w-8" />
          </TableCell>
          <TableCell>
            <Skeleton className="h-4 w-12" />
          </TableCell>
          <TableCell>
            <Skeleton className="h-4 w-20" />
          </TableCell>
        </TableRow>
      ))}
    </>
  );
}

function Pager({
  offset,
  count,
  limit,
  hasMore,
  onPageChange,
}: {
  offset: number;
  count: number;
  limit: number;
  hasMore: boolean;
  onPageChange: (offset: number) => void;
}) {
  return (
    <div className="mt-4 flex items-center justify-between">
      <p className="text-xs text-muted-foreground">
        {count > 0 ? `Showing ${offset + 1} - ${offset + count}` : "No tasks on this page"}
      </p>
      <div className="flex items-center gap-2">
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="Previous page"
          onClick={() => onPageChange(Math.max(0, offset - limit))}
          disabled={offset === 0}
        >
          <ChevronLeft className="h-4 w-4" />
        </Button>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="Next page"
          onClick={() => onPageChange(offset + limit)}
          disabled={!hasMore}
        >
          <ChevronRight className="h-4 w-4" />
        </Button>
      </div>
    </div>
  );
}

export function TaskTable({
  tasks,
  isLoading,
  offset,
  limit,
  hasMore,
  onPageChange,
}: TaskTableProps) {
  const navigate = useNavigate();
  const deleteTask = useDeleteTask();
  const deleteError = deleteTask.isError && (
    <ErrorNote compact className="mb-3">
      Could not delete task {truncateId(deleteTask.variables ?? "")}:{" "}
      {errorMessage(deleteTask.error)}
    </ErrorNote>
  );

  if (!isLoading && tasks.length === 0 && offset > 0) {
    // A page past the end (its tasks were deleted): keep the way back.
    return (
      <div>
        {deleteError}
        <div className="flex h-40 items-center justify-center rounded-lg border bg-card text-sm text-muted-foreground">
          No more tasks
        </div>
        <Pager offset={offset} count={0} limit={limit} hasMore={false} onPageChange={onPageChange} />
      </div>
    );
  }

  if (!isLoading && tasks.length === 0) {
    return (
      <div className="flex h-64 flex-col items-center justify-center rounded-lg border bg-card">
        <ListTodo className="h-10 w-10 text-muted-foreground/50" />
        <p className="mt-3 text-sm font-medium text-muted-foreground">No tasks found</p>
        <p className="mt-1 text-xs text-muted-foreground/70">
          Try adjusting your filters or create a new task
        </p>
      </div>
    );
  }

  return (
    <div>
      {deleteError}
      <div className="overflow-hidden rounded-lg border">
        <Table>
          <TableHeader>
            <TableRow className="hover:bg-transparent">
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                ID
              </TableHead>
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                Queue
              </TableHead>
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                Task Name
              </TableHead>
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                Status
              </TableHead>
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                Priority
              </TableHead>
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                Attempts
              </TableHead>
              <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                Created
              </TableHead>
              <TableHead className="w-12 px-4" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading ? (
              <SkeletonRows />
            ) : (
              tasks.map((task) => (
                <TableRow
                  key={task.id}
                  onClick={() => navigate(`/tasks/${encodeURIComponent(task.id)}`)}
                  className="cursor-pointer"
                >
                  <TableCell className="px-4 font-mono text-xs text-muted-foreground">
                    <Link
                      to={`/tasks/${encodeURIComponent(task.id)}`}
                      className="rounded-sm underline-offset-2 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      aria-label={`Open task ${task.task_name} (${task.id})`}
                      onClick={(e) => e.stopPropagation()}
                    >
                      {truncateId(task.id)}
                    </Link>
                  </TableCell>
                  <TableCell className="px-4 text-muted-foreground">
                    {task.queue_name}
                  </TableCell>
                  <TableCell className="px-4 font-medium">{task.task_name}</TableCell>
                  <TableCell className="px-4">
                    <TaskStatusBadge status={task.status} />
                  </TableCell>
                  <TableCell className="px-4 text-muted-foreground">
                    {task.priority}
                  </TableCell>
                  <TableCell className="px-4 text-muted-foreground">
                    {task.attempt_count}/{task.max_retries}
                  </TableCell>
                  <TableCell className="px-4 text-xs text-muted-foreground">
                    {formatRelative(task.created_at)}
                  </TableCell>
                  <TableCell className="px-4">
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="text-muted-foreground hover:text-destructive"
                      aria-label={`Delete task ${truncateId(task.id)}`}
                      disabled={deleteTask.isPending && deleteTask.variables === task.id}
                      onClick={(e) => {
                        e.stopPropagation();
                        if (window.confirm(`Delete task ${task.id.slice(0, 8)}...?`)) {
                          deleteTask.mutate(task.id);
                        }
                      }}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      <Pager
        offset={offset}
        count={tasks.length}
        limit={limit}
        hasMore={hasMore}
        onPageChange={onPageChange}
      />
    </div>
  );
}
