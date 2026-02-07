import { useEffect, useRef } from "react";
import { Radio, Trash2 } from "lucide-react";
import type { TaskEvent } from "@/api/types";
import { cn, statusDotColor, truncateId } from "@/lib/utils";
import {
  Card,
  CardHeader,
  CardTitle,
  CardAction,
  CardContent,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";

interface EventStreamProps {
  events: TaskEvent[];
  connected: boolean;
  onClear: () => void;
  maxHeight?: string;
}

function ConnectionIndicator({ connected }: { connected: boolean }) {
  return (
    <div className="flex items-center gap-2">
      <span className="relative flex h-2 w-2">
        {connected && (
          <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-75" />
        )}
        <span
          className={cn(
            "relative inline-flex h-2 w-2 rounded-full",
            connected ? "bg-emerald-400" : "bg-zinc-500"
          )}
        />
      </span>
      <span
        className={cn(
          "text-xs font-medium",
          connected ? "text-emerald-400" : "text-muted-foreground"
        )}
      >
        {connected ? "Live" : "Disconnected"}
      </span>
    </div>
  );
}

function EventRow({ event }: { event: TaskEvent }) {
  return (
    <div className="flex items-center gap-3 px-4 py-2.5 text-sm transition-colors hover:bg-accent/50">
      <span
        className={cn("h-2 w-2 shrink-0 rounded-full", statusDotColor(event.status))}
      />

      <span className="w-20 shrink-0 font-mono text-xs text-muted-foreground">
        {new Date(event.timestamp).toLocaleTimeString()}
      </span>

      <span className={cn(
        "inline-flex shrink-0 items-center rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase",
        statusDotColor(event.status).replace("bg-", "text-").replace("400", "400"),
        "border-current/20 bg-current/10"
      )}>
        {event.status}
      </span>

      <span className="shrink-0 text-xs text-muted-foreground">
        {event.queue_name}
      </span>

      <span className="ml-auto shrink-0 font-mono text-xs text-muted-foreground">
        {truncateId(event.task_id)}
      </span>
    </div>
  );
}

export function EventStream({
  events,
  connected,
  onClear,
  maxHeight = "600px",
}: EventStreamProps) {
  const viewportRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport) {
      viewport.scrollTop = 0;
    }
  }, [events.length]);

  return (
    <Card className="gap-0 py-0">
      <CardHeader className="py-4">
        <div className="flex items-center gap-3">
          <CardTitle className="flex items-center gap-2 text-base">
            <Radio className="h-4 w-4 text-muted-foreground" />
            Event Stream
          </CardTitle>
          <ConnectionIndicator connected={connected} />
          <span className="text-xs tabular-nums text-muted-foreground">
            {events.length} event{events.length !== 1 ? "s" : ""}
          </span>
        </div>
        <CardAction>
          <Button variant="ghost" size="sm" onClick={onClear}>
            <Trash2 />
            Clear
          </Button>
        </CardAction>
      </CardHeader>

      <CardContent className="p-0">
        <ScrollArea
          style={{ maxHeight }}
          ref={(node) => {
            if (node) {
              const viewport = node.querySelector(
                "[data-slot='scroll-area-viewport']"
              ) as HTMLDivElement | null;
              viewportRef.current = viewport;
            }
          }}
        >
          {events.length === 0 ? (
            <div className="flex h-48 flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
              <Radio className="h-5 w-5 opacity-40" />
              Waiting for events...
            </div>
          ) : (
            <div className="divide-y divide-border">
              {events.map((event, i) => (
                <EventRow
                  key={`${event.task_id}-${event.timestamp}-${i}`}
                  event={event}
                />
              ))}
            </div>
          )}
        </ScrollArea>
      </CardContent>
    </Card>
  );
}
