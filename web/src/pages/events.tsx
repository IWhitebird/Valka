import { useEvents } from "@/hooks/use-events";
import { EventStream } from "@/components/events/event-stream";

export function EventsPage() {
  const { events, connected, clear } = useEvents();

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">Events</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Live stream of task queue events via SSE
        </p>
      </div>

      <EventStream
        events={events}
        connected={connected}
        onClear={clear}
        maxHeight="calc(100vh - 200px)"
      />
    </div>
  );
}
