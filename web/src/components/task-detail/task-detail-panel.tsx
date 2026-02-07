import {
  XCircle,
  Clock,
  Hash,
  RefreshCw,
  Timer,
  Key,
  Calendar,
} from "lucide-react";
import type { Task } from "@/api/types";
import { formatDate } from "@/lib/utils";
import { useCancelTask } from "@/hooks/use-tasks";
import { TaskStatusBadge } from "@/components/tasks/task-status-badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";

interface TaskDetailPanelProps {
  task: Task;
}

function DetailRow({
  icon: Icon,
  label,
  value,
}: {
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  value: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-3 py-2.5">
      <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />
      <span className="w-36 shrink-0 text-sm text-muted-foreground">{label}</span>
      <span className="text-sm">{value}</span>
    </div>
  );
}

function JsonBlock({ label, data }: { label: string; data: unknown }) {
  return (
    <Card>
      <CardHeader className="pb-0">
        <CardTitle className="text-sm text-muted-foreground">{label}</CardTitle>
      </CardHeader>
      <CardContent>
        {data === null || data === undefined ? (
          <div className="rounded-md border bg-muted/30 px-4 py-3 font-mono text-sm text-muted-foreground">
            null
          </div>
        ) : (
          <pre className="overflow-x-auto rounded-md border bg-muted/30 px-4 py-3 font-mono text-sm">
            {JSON.stringify(data, null, 2)}
          </pre>
        )}
      </CardContent>
    </Card>
  );
}

export function TaskDetailPanel({ task }: TaskDetailPanelProps) {
  const cancelTask = useCancelTask();

  const canCancel =
    task.status === "PENDING" ||
    task.status === "DISPATCHING" ||
    task.status === "RUNNING";

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex items-start justify-between">
        <div>
          <h2 className="text-xl font-semibold">{task.task_name}</h2>
          <p className="mt-1 font-mono text-sm text-muted-foreground">{task.id}</p>
        </div>
        <div className="flex items-center gap-3">
          <TaskStatusBadge status={task.status} />
          {canCancel && (
            <Button
              variant="destructive"
              size="sm"
              onClick={() => cancelTask.mutate(task.id)}
              disabled={cancelTask.isPending}
            >
              <XCircle className="h-4 w-4" />
              {cancelTask.isPending ? "Cancelling..." : "Cancel"}
            </Button>
          )}
        </div>
      </div>

      {/* Details Card */}
      <Card>
        <CardHeader className="pb-0">
          <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
            Details
          </CardTitle>
        </CardHeader>
        <CardContent>
          <div className="divide-y divide-border">
            <DetailRow icon={Hash} label="Queue" value={task.queue_name} />
            <DetailRow icon={Hash} label="Priority" value={task.priority} />
            <DetailRow
              icon={RefreshCw}
              label="Attempts"
              value={`${task.attempt_count} / ${task.max_retries}`}
            />
            <DetailRow
              icon={Timer}
              label="Timeout"
              value={`${task.timeout_seconds}s`}
            />
            <DetailRow
              icon={Key}
              label="Idempotency Key"
              value={task.idempotency_key || "--"}
            />
            <DetailRow
              icon={Clock}
              label="Created"
              value={formatDate(task.created_at)}
            />
            <DetailRow
              icon={Clock}
              label="Updated"
              value={formatDate(task.updated_at)}
            />
            {task.scheduled_at && (
              <DetailRow
                icon={Calendar}
                label="Scheduled At"
                value={formatDate(task.scheduled_at)}
              />
            )}
          </div>
        </CardContent>
      </Card>

      {/* Error Section */}
      {task.error_message && (
        <Card className="border-destructive/30 bg-destructive/5">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm text-destructive">Error</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="font-mono text-sm text-destructive/90">{task.error_message}</p>
          </CardContent>
        </Card>
      )}

      <Separator />

      {/* JSON Blocks */}
      <div className="grid gap-6 lg:grid-cols-2">
        <JsonBlock label="Input" data={task.input} />
        <JsonBlock label="Output" data={task.output} />
      </div>

      <JsonBlock label="Metadata" data={task.metadata} />
    </div>
  );
}
