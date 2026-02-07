import { useState } from "react";
import { ChevronLeft, ChevronRight, ChevronDown, RefreshCw, Skull } from "lucide-react";
import { useDeadLetters } from "@/hooks/use-tasks";
import { truncateId, formatDate } from "@/lib/utils";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui/table";
import { cn } from "@/lib/utils";
import type { DeadLetter } from "@/api/types";

const PAGE_SIZE = 25;

function DeadLetterExpandedRow({ dl }: { dl: DeadLetter }) {
  return (
    <TableRow className="hover:bg-transparent">
      <TableCell colSpan={7} className="bg-muted/30 px-8 py-4">
        <div className="grid gap-4 lg:grid-cols-2">
          <div>
            <p className="mb-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">Input</p>
            <pre className="overflow-x-auto rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground">
              {dl.input ? JSON.stringify(dl.input, null, 2) : "null"}
            </pre>
          </div>
          <div>
            <p className="mb-2 text-xs font-medium uppercase tracking-wider text-muted-foreground">Metadata</p>
            <pre className="overflow-x-auto rounded-md border bg-background px-3 py-2 font-mono text-xs text-foreground">
              {dl.metadata ? JSON.stringify(dl.metadata, null, 2) : "null"}
            </pre>
          </div>
        </div>
      </TableCell>
    </TableRow>
  );
}

export function DeadLettersPage() {
  const [offset, setOffset] = useState(0);
  const [queueFilter, setQueueFilter] = useState("");
  const [expandedId, setExpandedId] = useState<number | null>(null);
  const navigate = useNavigate();

  const {
    data: deadLetters = [],
    isLoading,
    refetch,
  } = useDeadLetters({
    queue_name: queueFilter || undefined,
    limit: PAGE_SIZE,
    offset,
  });

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">
            Dead Letters
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Tasks that have exhausted all retry attempts
          </p>
        </div>
        <Button variant="outline" size="icon" onClick={() => refetch()}>
          <RefreshCw className="h-4 w-4" />
        </Button>
      </div>

      <div className="flex items-center gap-3">
        <Input
          type="text"
          placeholder="Filter by queue..."
          value={queueFilter}
          onChange={(e) => {
            setQueueFilter(e.target.value);
            setOffset(0);
          }}
          className="max-w-xs"
        />
      </div>

      {isLoading ? (
        <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
          Loading...
        </div>
      ) : deadLetters.length === 0 ? (
        <div className="flex h-64 flex-col items-center justify-center rounded-lg border">
          <div className="mb-3 flex h-10 w-10 items-center justify-center rounded-lg bg-muted">
            <Skull className="h-5 w-5 text-muted-foreground" />
          </div>
          <p className="text-sm text-muted-foreground">No dead letters</p>
          <p className="mt-1 text-xs text-muted-foreground/60">
            Tasks that fail all retries will appear here
          </p>
        </div>
      ) : (
        <div>
          <div className="rounded-lg border">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead className="w-8" />
                  <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Task ID
                  </TableHead>
                  <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Queue
                  </TableHead>
                  <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Task Name
                  </TableHead>
                  <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Attempts
                  </TableHead>
                  <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Error
                  </TableHead>
                  <TableHead className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                    Failed At
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {deadLetters.map((dl) => (
                  <>
                    <TableRow
                      key={dl.id}
                      className="cursor-pointer"
                    >
                      <TableCell className="w-8 px-2">
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          onClick={(e) => {
                            e.stopPropagation();
                            setExpandedId(expandedId === dl.id ? null : dl.id);
                          }}
                        >
                          <ChevronDown
                            className={cn(
                              "h-3 w-3 transition-transform",
                              expandedId === dl.id && "rotate-180"
                            )}
                          />
                        </Button>
                      </TableCell>
                      <TableCell
                        className="font-mono text-xs text-muted-foreground"
                        onClick={() => navigate(`/tasks/${dl.task_id}`)}
                      >
                        {truncateId(dl.task_id)}
                      </TableCell>
                      <TableCell
                        className="text-foreground"
                        onClick={() => navigate(`/tasks/${dl.task_id}`)}
                      >
                        {dl.queue_name}
                      </TableCell>
                      <TableCell
                        className="font-medium text-foreground"
                        onClick={() => navigate(`/tasks/${dl.task_id}`)}
                      >
                        {dl.task_name}
                      </TableCell>
                      <TableCell
                        className="text-muted-foreground"
                        onClick={() => navigate(`/tasks/${dl.task_id}`)}
                      >
                        {dl.attempt_count}
                      </TableCell>
                      <TableCell
                        className="max-w-xs truncate text-xs text-red-400"
                        onClick={() => navigate(`/tasks/${dl.task_id}`)}
                      >
                        {dl.error_message || "--"}
                      </TableCell>
                      <TableCell
                        className="text-xs text-muted-foreground"
                        onClick={() => navigate(`/tasks/${dl.task_id}`)}
                      >
                        {formatDate(dl.created_at)}
                      </TableCell>
                    </TableRow>
                    {expandedId === dl.id && (
                      <DeadLetterExpandedRow key={`${dl.id}-expanded`} dl={dl} />
                    )}
                  </>
                ))}
              </TableBody>
            </Table>
          </div>

          <div className="mt-4 flex items-center justify-between">
            <p className="text-xs text-muted-foreground">
              Showing {offset + 1} - {offset + deadLetters.length}
            </p>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="icon-sm"
                onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
                disabled={offset === 0}
              >
                <ChevronLeft className="h-4 w-4" />
              </Button>
              <Button
                variant="outline"
                size="icon-sm"
                onClick={() => setOffset(offset + PAGE_SIZE)}
                disabled={deadLetters.length < PAGE_SIZE}
              >
                <ChevronRight className="h-4 w-4" />
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
