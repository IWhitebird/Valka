import { useState } from "react";
import { Radio, Send, CheckCircle2, Truck, Clock } from "lucide-react";
import type { TaskSignal, TaskStatus } from "@/api/types";
import { useTaskSignals, useSendSignal } from "@/hooks/use-tasks";
import { formatDate, truncateId, cn } from "@/lib/utils";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";
import { Skeleton } from "@/components/ui/skeleton";

interface TaskSignalsPanelProps {
  taskId: string;
  taskStatus: TaskStatus;
}

function signalStatusColor(status: string) {
  switch (status) {
    case "PENDING":
      return "bg-zinc-500/10 text-zinc-400 border-zinc-500/20";
    case "DELIVERED":
      return "bg-blue-500/10 text-blue-400 border-blue-500/20";
    case "ACKNOWLEDGED":
      return "bg-emerald-500/10 text-emerald-400 border-emerald-500/20";
    default:
      return "bg-zinc-500/10 text-zinc-400 border-zinc-500/20";
  }
}

function SignalStatusIcon({ status }: { status: string }) {
  switch (status) {
    case "PENDING":
      return <Clock className="h-3 w-3" />;
    case "DELIVERED":
      return <Truck className="h-3 w-3" />;
    case "ACKNOWLEDGED":
      return <CheckCircle2 className="h-3 w-3" />;
    default:
      return <Clock className="h-3 w-3" />;
  }
}

function SkeletonRows() {
  return (
    <>
      {Array.from({ length: 2 }).map((_, i) => (
        <TableRow key={i}>
          <TableCell><Skeleton className="h-4 w-16" /></TableCell>
          <TableCell><Skeleton className="h-4 w-24" /></TableCell>
          <TableCell><Skeleton className="h-5 w-20 rounded-full" /></TableCell>
          <TableCell><Skeleton className="h-4 w-28" /></TableCell>
          <TableCell><Skeleton className="h-4 w-28" /></TableCell>
        </TableRow>
      ))}
    </>
  );
}

export function TaskSignalsPanel({ taskId, taskStatus }: TaskSignalsPanelProps) {
  const { data: signals = [], isLoading } = useTaskSignals(taskId);
  const sendSignal = useSendSignal();
  const [signalName, setSignalName] = useState("");
  const [payload, setPayload] = useState("");
  const [payloadError, setPayloadError] = useState<string | null>(null);

  const canSendSignal =
    taskStatus === "PENDING" ||
    taskStatus === "DISPATCHING" ||
    taskStatus === "RUNNING" ||
    taskStatus === "RETRY";

  function handleSendSignal() {
    if (!signalName.trim()) return;

    let parsedPayload: Record<string, unknown> | undefined;
    if (payload.trim()) {
      try {
        parsedPayload = JSON.parse(payload.trim());
        setPayloadError(null);
      } catch {
        setPayloadError("Invalid JSON");
        return;
      }
    }

    sendSignal.mutate(
      {
        taskId,
        request: {
          signal_name: signalName.trim(),
          payload: parsedPayload,
        },
      },
      {
        onSuccess: () => {
          setSignalName("");
          setPayload("");
          setPayloadError(null);
        },
      },
    );
  }

  return (
    <div className="space-y-4">
      <h3 className="text-lg font-semibold text-foreground">Signals</h3>

      {/* Send Signal Form */}
      {canSendSignal && (
        <Card>
          <CardHeader className="pb-0">
            <CardTitle className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              Send Signal
            </CardTitle>
          </CardHeader>
          <CardContent>
            <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
              <div className="flex-1 space-y-1.5">
                <Label htmlFor="signal-name" className="text-xs text-muted-foreground">
                  Signal Name
                </Label>
                <Input
                  id="signal-name"
                  placeholder="e.g. pause, resume, shutdown"
                  value={signalName}
                  onChange={(e) => setSignalName(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") handleSendSignal();
                  }}
                />
              </div>
              <div className="flex-1 space-y-1.5">
                <Label htmlFor="signal-payload" className="text-xs text-muted-foreground">
                  Payload (JSON, optional)
                </Label>
                <Textarea
                  id="signal-payload"
                  placeholder='{"key": "value"}'
                  value={payload}
                  onChange={(e) => {
                    setPayload(e.target.value);
                    setPayloadError(null);
                  }}
                  rows={1}
                  className={cn("resize-none font-mono text-xs", payloadError && "border-red-500")}
                />
                {payloadError && (
                  <p className="text-xs text-red-400">{payloadError}</p>
                )}
              </div>
              <Button
                onClick={handleSendSignal}
                disabled={!signalName.trim() || sendSignal.isPending}
                size="sm"
              >
                <Send className="mr-1.5 h-3.5 w-3.5" />
                {sendSignal.isPending ? "Sending..." : "Send"}
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Signals Table */}
      {isLoading ? (
        <div className="overflow-hidden rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  ID
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Name
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Status
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Payload
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Created
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <SkeletonRows />
            </TableBody>
          </Table>
        </div>
      ) : signals.length === 0 ? (
        <div className="flex h-32 flex-col items-center justify-center rounded-lg border bg-card">
          <Radio className="h-8 w-8 text-muted-foreground/50" />
          <p className="mt-2 text-sm text-muted-foreground">No signals sent</p>
        </div>
      ) : (
        <div className="overflow-hidden rounded-lg border">
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  ID
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Name
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Status
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Payload
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Created
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Delivered
                </TableHead>
                <TableHead className="px-4 text-xs uppercase tracking-wider text-muted-foreground">
                  Acknowledged
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {signals.map((signal: TaskSignal) => (
                <TableRow key={signal.id}>
                  <TableCell className="px-4 font-mono text-xs text-muted-foreground">
                    {truncateId(signal.id)}
                  </TableCell>
                  <TableCell className="px-4 font-medium text-foreground">
                    {signal.signal_name}
                  </TableCell>
                  <TableCell className="px-4">
                    <Badge
                      variant="outline"
                      className={cn(
                        "inline-flex items-center gap-1 text-xs",
                        signalStatusColor(signal.status),
                      )}
                    >
                      <SignalStatusIcon status={signal.status} />
                      {signal.status}
                    </Badge>
                  </TableCell>
                  <TableCell className="max-w-[200px] truncate px-4 font-mono text-xs text-muted-foreground">
                    {signal.payload ? JSON.stringify(signal.payload) : "--"}
                  </TableCell>
                  <TableCell className="px-4 text-xs text-muted-foreground">
                    {formatDate(signal.created_at)}
                  </TableCell>
                  <TableCell className="px-4 text-xs text-muted-foreground">
                    {signal.delivered_at ? formatDate(signal.delivered_at) : "--"}
                  </TableCell>
                  <TableCell className="px-4 text-xs text-muted-foreground">
                    {signal.acknowledged_at ? formatDate(signal.acknowledged_at) : "--"}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
      )}
    </div>
  );
}
