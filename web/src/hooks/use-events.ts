import { useState, useEffect, useCallback, useRef } from "react";
import { subscribeEvents } from "@/api/events";
import type { TaskEvent } from "@/api/types";

const MAX_EVENTS = 200;

export function useEvents() {
  const [events, setEvents] = useState<TaskEvent[]>([]);
  const [connected, setConnected] = useState(false);
  const cleanupRef = useRef<(() => void) | null>(null);

  const connect = useCallback(() => {
    if (cleanupRef.current) {
      cleanupRef.current();
    }

    const unsubscribe = subscribeEvents(
      (event) => {
        setConnected(true);
        setEvents((prev) => {
          const next = [event, ...prev];
          return next.length > MAX_EVENTS ? next.slice(0, MAX_EVENTS) : next;
        });
      },
      () => {
        setConnected(false);
      },
    );

    cleanupRef.current = unsubscribe;
  }, []);

  useEffect(() => {
    connect();
    return () => {
      cleanupRef.current?.();
    };
  }, [connect]);

  const clear = useCallback(() => {
    setEvents([]);
  }, []);

  return { events, connected, clear };
}
