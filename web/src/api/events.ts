import type { TaskEvent, TaskStatus, RawTaskEvent } from "./types";

// Proto status enum values → string status
const STATUS_MAP: Record<number, TaskStatus> = {
  0: "PENDING",
  1: "PENDING",
  2: "DISPATCHING",
  3: "RUNNING",
  4: "COMPLETED",
  5: "FAILED",
  6: "RETRY",
  7: "DEAD_LETTER",
  8: "CANCELLED",
};

function parseRawEvent(raw: RawTaskEvent): TaskEvent {
  return {
    event_id: raw.event_id,
    task_id: raw.task_id,
    queue_name: raw.queue_name,
    status: STATUS_MAP[raw.new_status] ?? "PENDING",
    timestamp: new Date(raw.timestamp_ms).toISOString(),
  };
}

export function subscribeEvents(
  onEvent: (event: TaskEvent) => void,
  onError?: (error: Event) => void,
): () => void {
  const eventSource = new EventSource("/api/v1/events");

  eventSource.onmessage = (event) => {
    try {
      const raw = JSON.parse(event.data) as RawTaskEvent;
      onEvent(parseRawEvent(raw));
    } catch {
      // Ignore parse errors for malformed events
    }
  };

  eventSource.onerror = (error) => {
    onError?.(error);
  };

  return () => {
    eventSource.close();
  };
}
