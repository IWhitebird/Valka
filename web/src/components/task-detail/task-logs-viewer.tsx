import { useEffect, useRef } from "react";
import { Terminal } from "lucide-react";
import { useTaskRunLogs } from "@/hooks/use-tasks";
import { cn } from "@/lib/utils";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";

interface TaskLogsViewerProps {
  taskId: string;
  runId: string;
}

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

export function TaskLogsViewer({ taskId, runId }: TaskLogsViewerProps) {
  const { data: logs, isLoading } = useTaskRunLogs(taskId, runId);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (bottomRef.current) {
      bottomRef.current.scrollIntoView({ behavior: "smooth" });
    }
  }, [logs]);

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

  if (!logs || logs.length === 0) {
    return (
      <div className="flex h-48 flex-col items-center justify-center rounded-lg border border-border bg-black">
        <Terminal className="h-8 w-8 text-muted-foreground/40" />
        <p className="mt-2 text-sm text-muted-foreground">No logs available</p>
      </div>
    );
  }

  return (
    <ScrollArea className="h-96 rounded-lg border border-border bg-black">
      <div className="p-4 font-mono text-xs">
        {logs.map((log) => (
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
        <div ref={bottomRef} />
      </div>
    </ScrollArea>
  );
}
