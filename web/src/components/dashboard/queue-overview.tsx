import { Inbox } from "lucide-react";
import type { Task } from "@/api/types";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
} from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Separator } from "@/components/ui/separator";

interface QueueOverviewProps {
  tasks: Task[];
  isLoading: boolean;
}

interface QueueStats {
  name: string;
  total: number;
  pending: number;
  running: number;
  completed: number;
  failed: number;
}

const legendItems = [
  { label: "Completed", color: "bg-emerald-500" },
  { label: "Running", color: "bg-sky-500" },
  { label: "Pending", color: "bg-zinc-500" },
  { label: "Failed", color: "bg-red-500" },
] as const;

function QueueBar({ queue }: { queue: QueueStats }) {
  if (queue.total === 0) return null;

  const segments = [
    { count: queue.completed, color: "bg-emerald-500" },
    { count: queue.running, color: "bg-sky-500" },
    { count: queue.pending, color: "bg-zinc-500" },
    { count: queue.failed, color: "bg-red-500" },
  ];

  return (
    <div className="flex h-2 w-full overflow-hidden rounded-full bg-secondary">
      {segments.map(
        (segment, i) =>
          segment.count > 0 && (
            <div
              key={i}
              className={`${segment.color} transition-all duration-500`}
              style={{ width: `${(segment.count / queue.total) * 100}%` }}
            />
          )
      )}
    </div>
  );
}

function QueueItem({ queue }: { queue: QueueStats }) {
  return (
    <div className="space-y-2.5">
      <div className="flex items-baseline justify-between">
        <span className="text-sm font-medium">{queue.name}</span>
        <span className="text-muted-foreground text-xs tabular-nums">
          {queue.total} task{queue.total !== 1 ? "s" : ""}
        </span>
      </div>

      <QueueBar queue={queue} />

      <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs">
        {queue.completed > 0 && (
          <span className="text-emerald-400">{queue.completed} completed</span>
        )}
        {queue.running > 0 && (
          <span className="text-sky-400">{queue.running} running</span>
        )}
        {queue.pending > 0 && (
          <span className="text-muted-foreground">{queue.pending} pending</span>
        )}
        {queue.failed > 0 && (
          <span className="text-red-400">{queue.failed} failed</span>
        )}
      </div>
    </div>
  );
}

function QueueOverviewSkeleton() {
  return (
    <Card>
      <CardHeader>
        <Skeleton className="h-5 w-32" />
        <Skeleton className="h-4 w-48" />
      </CardHeader>
      <CardContent className="space-y-6">
        {Array.from({ length: 3 }).map((_, i) => (
          <div key={i} className="space-y-2.5">
            <div className="flex items-center justify-between">
              <Skeleton className="h-4 w-28" />
              <Skeleton className="h-3 w-16" />
            </div>
            <Skeleton className="h-2 w-full rounded-full" />
            <div className="flex gap-4">
              <Skeleton className="h-3 w-20" />
              <Skeleton className="h-3 w-16" />
            </div>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}

export function QueueOverview({ tasks, isLoading }: QueueOverviewProps) {
  if (isLoading) {
    return <QueueOverviewSkeleton />;
  }

  const queueMap = new Map<string, QueueStats>();

  for (const task of tasks) {
    const existing = queueMap.get(task.queue_name);
    if (existing) {
      existing.total++;
      if (task.status === "PENDING") existing.pending++;
      if (task.status === "RUNNING") existing.running++;
      if (task.status === "COMPLETED") existing.completed++;
      if (task.status === "FAILED" || task.status === "DEAD_LETTER") existing.failed++;
    } else {
      queueMap.set(task.queue_name, {
        name: task.queue_name,
        total: 1,
        pending: task.status === "PENDING" ? 1 : 0,
        running: task.status === "RUNNING" ? 1 : 0,
        completed: task.status === "COMPLETED" ? 1 : 0,
        failed: task.status === "FAILED" || task.status === "DEAD_LETTER" ? 1 : 0,
      });
    }
  }

  const queues = Array.from(queueMap.values()).sort((a, b) => b.total - a.total);

  return (
    <Card>
      <CardHeader>
        <CardTitle>Queue Overview</CardTitle>
        <CardDescription>
          {queues.length > 0
            ? `${queues.length} active queue${queues.length !== 1 ? "s" : ""}`
            : "No queues active"}
        </CardDescription>
      </CardHeader>
      <CardContent>
        {queues.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-10 text-center">
            <div className="bg-muted mb-3 flex h-10 w-10 items-center justify-center rounded-lg">
              <Inbox className="text-muted-foreground h-5 w-5" />
            </div>
            <p className="text-muted-foreground text-sm">
              No queues found. Tasks will appear here once created.
            </p>
          </div>
        ) : (
          <div className="space-y-2">
            <div className="flex flex-wrap gap-x-4 gap-y-1 pb-1">
              {legendItems.map((item) => (
                <div key={item.label} className="flex items-center gap-1.5">
                  <span className={`h-2 w-2 rounded-full ${item.color}`} />
                  <span className="text-muted-foreground text-xs">{item.label}</span>
                </div>
              ))}
            </div>

            <Separator />

            <div className="space-y-5 pt-1">
              {queues.map((queue) => (
                <QueueItem key={queue.name} queue={queue} />
              ))}
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
