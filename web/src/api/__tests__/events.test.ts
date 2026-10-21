import { afterEach, describe, expect, it, vi } from "vitest";
import { parseRawEvent, subscribeEvents } from "../events";
import { credentials } from "@/lib/credentials";

/** The few EventSource members `subscribeEvents` uses. */
class FakeEventSource {
  static last: FakeEventSource | null = null;
  url: string;
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  closed = false;
  constructor(url: string) {
    this.url = url;
    FakeEventSource.last = this;
  }
  close() {
    this.closed = true;
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  credentials.setApiKey(null);
});

describe("parseRawEvent", () => {
  it("maps proto statuses, and 0 (a task that left the system) to DELETED", () => {
    const raw = { event_id: "e", task_id: "t", queue_name: "q", timestamp_ms: 0 };
    expect(parseRawEvent({ ...raw, new_status: 4 }).status).toBe("COMPLETED");
    expect(parseRawEvent({ ...raw, new_status: 6 }).status).toBe("RETRY");
    expect(parseRawEvent({ ...raw, new_status: 0 }).status).toBe("DELETED");
    expect(parseRawEvent({ ...raw, new_status: 42 }).status).toBe("UNKNOWN");
  });
});

describe("subscribeEvents", () => {
  it("reports the connection as soon as the stream opens, before any event", () => {
    vi.stubGlobal("EventSource", FakeEventSource);
    credentials.setApiKey("k/1");
    const onOpen = vi.fn();
    const onEvent = vi.fn();
    const onError = vi.fn();
    const unsubscribe = subscribeEvents({ onEvent, onOpen, onError });
    const es = FakeEventSource.last!;
    expect(es.url).toBe("/api/v1/events?access_token=k%2F1");

    es.onopen?.();
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onEvent).not.toHaveBeenCalled();

    es.onmessage?.({
      data: JSON.stringify({ event_id: "e", task_id: "t", queue_name: "q", new_status: 3, timestamp_ms: 0 }),
    });
    expect(onEvent).toHaveBeenCalledWith(expect.objectContaining({ status: "RUNNING" }));

    es.onerror?.(new Event("error"));
    expect(onError).toHaveBeenCalledTimes(1);
    unsubscribe();
    expect(es.closed).toBe(true);
  });
});
