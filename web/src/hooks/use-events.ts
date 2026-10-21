import { useState, useEffect, useCallback } from "react";
import { subscribeEvents } from "@/api/events";
import type { TaskEvent } from "@/api/types";

const MAX_EVENTS = 200;

export function useEvents() {
  const [events, setEvents] = useState<TaskEvent[]>([]);
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    return subscribeEvents({
      onOpen: () => setConnected(true),
      onEvent: (event) => {
        setConnected(true);
        setEvents((prev) => {
          const next = [event, ...prev];
          return next.length > MAX_EVENTS ? next.slice(0, MAX_EVENTS) : next;
        });
      },
      onError: () => setConnected(false),
    });
  }, []);

  const clear = useCallback(() => {
    setEvents([]);
  }, []);

  return { events, connected, clear };
}
