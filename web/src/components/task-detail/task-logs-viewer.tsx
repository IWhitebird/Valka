import { useEffect, useRef } from "react";
import { Terminal } from "lucide-react";
import { errorMessage } from "@/api/client";
import { useLoadEarlierLogs, useTaskRunLogs } from "@/hooks/use-tasks";
import { earlierCount } from "@/lib/logs";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { ErrorNote } from "@/components/ui/error-note";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";

interface TaskLogsViewerProps {
  taskId: string;
  runId: string;
  /** The run is over: polling stops once no new lines arrive. */
  runFinished: boolean;
}

/** Pixels from the bottom that still count as "following the tail". */
const STICK_THRESHOLD = 24;

function logLevelVariant(level: string) {
  switch (level.toUpperCase()) {
    case "ERROR":
      return "text-red-400 border-red-500/30 bg-red-500/10";
    case "WARN":
    case "WARNING":
      return "text-amber-400 border-amber-500/30 bg-amber-500/10";
    case "INFO":
      return "text-blue-400 border-blue-500/30 bg-blue-500/10";
    case "DEBUG":
      return "text-zinc-500 border-zinc-500/30 bg-zinc-500/10";
    default:
      return "text-zinc-400 border-zinc-500/30 bg-zinc-500/10";
  }
}

function formatTimestamp(timestampMs: number): string {
  return new Date(timestampMs).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  });
}

function viewportOf(wrapper: HTMLElement | null): HTMLElement | null {
  return wrapper?.querySelector<HTMLElement>("[data-slot='scroll-area-viewport']") ?? null;
}

export function TaskLogsViewer({ taskId, runId, runFinished }: TaskLogsViewerProps) {
  const { data: logWindow, isLoading, error } = useTaskRunLogs(taskId, runId, runFinished);
  const loadEarlier = useLoadEarlierLogs(taskId, runId);
  const wrapperRef = useRef<HTMLDivElement>(null);
  // Follow new lines only while the reader is at the bottom of the log.
  const stickRef = useRef(true);
  const lines = logWindow?.lines ?? [];
  const hasLines = lines.length > 0;
  const lastId = logWindow?.lastId ?? 0;
  const earlier = logWindow ? earlierCount(logWindow) : 0;

  useEffect(() => {
    const viewport = viewportOf(wrapperRef.current);
    if (!viewport) return;
    const onScroll = () => {
      stickRef.current =
        viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight < STICK_THRESHOLD;
    };
    viewport.addEventListener("scroll", onScroll);
    return () => viewport.removeEventListener("scroll", onScroll);
  }, [hasLines]);

  useEffect(() => {
    // Scroll the log box only, never the page around it.
    const viewport = viewportOf(wrapperRef.current);
    if (viewport && stickRef.current) viewport.scrollTop = viewport.scrollHeight;
  }, [lastId]);

  if (isLoading) {
    return (
      <div className="flex h-48 flex-col gap-2 rounded-lg border border-border bg-black p-4">
        <Skeleton className="h-4 w-3/4 bg-zinc-800" />
        <Skeleton className="h-4 w-1/2 bg-zinc-800" />
        <Skeleton className="h-4 w-2/3 bg-zinc-800" />
        <Skeleton className="h-4 w-1/3 bg-zinc-800" />
      </div>
    );
  }

  if (!hasLines) {
    if (error) return <ErrorNote>Could not load logs: {errorMessage(error)}</ErrorNote>;
    return (
      <div className="flex h-48 flex-col items-center justify-center rounded-lg border border-border bg-black">
        <Terminal className="h-8 w-8 text-muted-foreground/40" />
        <p className="mt-2 text-sm text-muted-foreground">No logs available</p>
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {error && <ErrorNote compact>Could not refresh logs: {errorMessage(error)}</ErrorNote>}
      <div ref={wrapperRef}>
        <ScrollArea className="h-96 rounded-lg border border-border bg-black">
          <div className="p-4 font-mono text-xs">
            {earlier > 0 && (
              <div className="mb-2 flex flex-wrap items-center gap-3 border-b border-zinc-800 pb-2 text-zinc-500">
                <span>
                  {earlier.toLocaleString()} earlier line{earlier === 1 ? "" : "s"} not shown
                </span>
                <Button
                  variant="outline"
                  size="sm"
                  className="h-6 px-2 text-xs"
                  disabled={loadEarlier.isPending}
                  onClick={() => loadEarlier.mutate()}
                >
                  {loadEarlier.isPending ? "Loading…" : "Load earlier"}
                </Button>
                {loadEarlier.isError && (
                  <span role="alert" className="text-red-400">
                    {errorMessage(loadEarlier.error)}
                  </span>
                )}
              </div>
            )}
            {lines.map((log) => (
              <div key={log.id} className="flex items-start gap-3 py-0.5 leading-5">
                <span className="shrink-0 tabular-nums text-zinc-600">
                  {formatTimestamp(log.timestamp_ms)}
                </span>
                <Badge
                  variant="outline"
                  className={cn(
                    "shrink-0 rounded px-1.5 py-0 text-[10px] font-semibold uppercase",
                    logLevelVariant(log.level),
                  )}
                >
                  {log.level}
                </Badge>
                <span className="text-zinc-300">{log.message}</span>
              </div>
            ))}
          </div>
        </ScrollArea>
      </div>
    </div>
  );
}
