import { credentials } from "@/lib/credentials";
import type { EventStatus, TaskEvent, RawTaskEvent } from "./types";

// Proto status enum values → string status. The server sends 0 (UNSPECIFIED) when a task
// leaves the system: deleted, or removed by "Clear All".
const STATUS_MAP: Record<number, EventStatus> = {
  0: "DELETED",
  1: "PENDING",
  2: "DISPATCHING",
  3: "RUNNING",
  4: "COMPLETED",
  5: "FAILED",
  6: "RETRY",
  7: "DEAD_LETTER",
  8: "CANCELLED",
};

export function parseRawEvent(raw: RawTaskEvent): TaskEvent {
  return {
    event_id: raw.event_id,
    task_id: raw.task_id,
    queue_name: raw.queue_name,
    status: STATUS_MAP[raw.new_status] ?? "UNKNOWN",
    timestamp: new Date(raw.timestamp_ms).toISOString(),
  };
}

export interface EventHandlers {
  onEvent: (event: TaskEvent) => void;
  /** The stream is connected (also after an automatic reconnect). */
  onOpen?: () => void;
  onError?: (error: Event) => void;
}

export function subscribeEvents({ onEvent, onOpen, onError }: EventHandlers): () => void {
  // EventSource cannot send headers: the server also takes the key as ?access_token=.
  const apiKey = credentials.apiKey();
  const eventSource = new EventSource(
    apiKey ? `/api/v1/events?access_token=${encodeURIComponent(apiKey)}` : "/api/v1/events",
  );

  eventSource.onopen = () => {
    onOpen?.();
  };

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
