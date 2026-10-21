import { Fragment, useState } from "react";
import { ChevronLeft, ChevronRight, ChevronDown, RefreshCw, Skull } from "lucide-react";
import { useDeadLetters } from "@/hooks/use-tasks";
import { truncateId, formatDate } from "@/lib/utils";
import { Link, useNavigate } from "react-router-dom";
import { errorMessage } from "@/api/client";
import { Button } from "@/components/ui/button";
import { ErrorNote } from "@/components/ui/error-note";
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
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const navigate = useNavigate();

  // One more than a page: whether it comes back tells if there is a next page.
  const { data, isLoading, error, refetch } = useDeadLetters({
    queue_name: queueFilter || undefined,
    limit: PAGE_SIZE + 1,
    offset,
  });
  const deadLetters = (data ?? []).slice(0, PAGE_SIZE);
  const hasMore = (data?.length ?? 0) > PAGE_SIZE;
  const openTask = (taskId: string) => navigate(`/tasks/${encodeURIComponent(taskId)}`);

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
        <Button variant="outline" size="icon" aria-label="Refresh dead letters" onClick={() => refetch()}>
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
          aria-label="Filter by queue"
        />
      </div>

      {error && data && (
        <ErrorNote compact>
          Could not refresh dead letters, showing the last answer: {errorMessage(error)}
        </ErrorNote>
      )}

      {isLoading ? (
        <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
          Loading...
        </div>
      ) : error && !data ? (
        <ErrorNote>Could not load dead letters: {errorMessage(error)}</ErrorNote>
      ) : deadLetters.length === 0 && offset > 0 ? (
        <div className="flex h-40 flex-col items-center justify-center gap-3 rounded-lg border text-sm text-muted-foreground">
          No more dead letters
          <Button variant="outline" size="sm" onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}>
            <ChevronLeft className="h-4 w-4" />
            Previous page
          </Button>
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
                  <Fragment key={dl.id}>
                    <TableRow className="cursor-pointer">
                      <TableCell className="w-8 px-2">
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          aria-label={`${expandedId === dl.id ? "Hide" : "Show"} input and metadata of task ${truncateId(dl.task_id)}`}
                          aria-expanded={expandedId === dl.id}
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
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        <Link
                          to={`/tasks/${encodeURIComponent(dl.task_id)}`}
                          className="rounded-sm underline-offset-2 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                          aria-label={`Open task ${dl.task_name} (${dl.task_id})`}
                        >
                          {truncateId(dl.task_id)}
                        </Link>
                      </TableCell>
                      <TableCell
                        className="text-foreground"
                        onClick={() => openTask(dl.task_id)}
                      >
                        {dl.queue_name}
                      </TableCell>
                      <TableCell
                        className="font-medium text-foreground"
                        onClick={() => openTask(dl.task_id)}
                      >
                        {dl.task_name}
                      </TableCell>
                      <TableCell
                        className="text-muted-foreground"
                        onClick={() => openTask(dl.task_id)}
                      >
                        {dl.attempt_count}
                      </TableCell>
                      <TableCell
                        className="max-w-xs truncate text-xs text-red-400"
                        onClick={() => openTask(dl.task_id)}
                      >
                        {dl.error_message || "--"}
                      </TableCell>
                      <TableCell
                        className="text-xs text-muted-foreground"
                        onClick={() => openTask(dl.task_id)}
                      >
                        {formatDate(dl.created_at)}
                      </TableCell>
                    </TableRow>
                    {expandedId === dl.id && <DeadLetterExpandedRow dl={dl} />}
                  </Fragment>
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
                aria-label="Previous page"
                onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
                disabled={offset === 0}
              >
                <ChevronLeft className="h-4 w-4" />
              </Button>
              <Button
                variant="outline"
                size="icon-sm"
                aria-label="Next page"
                onClick={() => setOffset(offset + PAGE_SIZE)}
                disabled={!hasMore}
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
