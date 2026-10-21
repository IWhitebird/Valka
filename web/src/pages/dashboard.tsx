import { useTasks } from "@/hooks/use-tasks";
import { useEvents } from "@/hooks/use-events";
import { useClusterOverview } from "@/hooks/use-cluster";
import { sumTaskCounts } from "@/lib/cluster";
import { ClusterStrip } from "@/components/dashboard/cluster-strip";
import { StatsCards } from "@/components/dashboard/stats-cards";
import { QueueOverview } from "@/components/dashboard/queue-overview";
import { EventStream } from "@/components/events/event-stream";

/** How many recent tasks the queue bars describe (no endpoint counts tasks per queue). */
const RECENT_TASKS = 200;

export function DashboardPage() {
  const overview = useClusterOverview();
  const counts = overview.data ? sumTaskCounts(overview.data.nodes) : undefined;
  const recent = useTasks({ limit: RECENT_TASKS });
  const { events, connected, clear } = useEvents();

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">Dashboard</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Overview of your task queue system
        </p>
      </div>

      <StatsCards counts={counts} isLoading={overview.isLoading} error={overview.error} />

      <ClusterStrip />

      <div className="grid gap-6 lg:grid-cols-2">
        <QueueOverview
          tasks={recent.data ?? []}
          isLoading={recent.isLoading}
          error={recent.data ? null : recent.error}
        />
        <EventStream
          events={events.slice(0, 50)}
          connected={connected}
          onClear={clear}
          maxHeight="340px"
        />
      </div>
    </div>
  );
}
