import { useTasks } from "@/hooks/use-tasks";
import { useEvents } from "@/hooks/use-events";
import { StatsCards } from "@/components/dashboard/stats-cards";
import { QueueOverview } from "@/components/dashboard/queue-overview";
import { EventStream } from "@/components/events/event-stream";

export function DashboardPage() {
  const { data: tasks = [], isLoading } = useTasks({ limit: 500 });
  const { events, connected, clear } = useEvents();

  return (
    <div className="space-y-8">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">Dashboard</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Overview of your task queue system
        </p>
      </div>

      <StatsCards tasks={tasks} isLoading={isLoading} />

      <div className="grid gap-6 lg:grid-cols-2">
        <QueueOverview tasks={tasks} isLoading={isLoading} />
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
