import { useNavigate } from "react-router-dom";
import { ChevronLeft, ChevronRight, ListTodo } from "lucide-react";
import type { Task } from "@/api/types";
import { truncateId, formatRelative } from "@/lib/utils";
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
import { Skeleton } from "@/components/ui/skeleton";

interface TaskTableProps {
  tasks: Task[];
  isLoading: boolean;
  offset: number;
  limit: number;
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

export function TaskTable({
  tasks,
  isLoading,
  offset,
  limit,
  onPageChange,
}: TaskTableProps) {
  const navigate = useNavigate();

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
            </TableRow>
          </TableHeader>
          <TableBody>
            {isLoading ? (
              <SkeletonRows />
            ) : (
              tasks.map((task) => (
                <TableRow
                  key={task.id}
                  onClick={() => navigate(`/tasks/${task.id}`)}
                  className="cursor-pointer"
                >
                  <TableCell className="px-4 font-mono text-xs text-muted-foreground">
                    {truncateId(task.id)}
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
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>

      <div className="mt-4 flex items-center justify-between">
        <p className="text-xs text-muted-foreground">
          Showing {offset + 1} - {offset + tasks.length}
        </p>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="icon-sm"
            onClick={() => onPageChange(Math.max(0, offset - limit))}
            disabled={offset === 0}
          >
            <ChevronLeft className="h-4 w-4" />
          </Button>
          <Button
            variant="outline"
            size="icon-sm"
            onClick={() => onPageChange(offset + limit)}
            disabled={tasks.length < limit}
          >
            <ChevronRight className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </div>
  );
}
