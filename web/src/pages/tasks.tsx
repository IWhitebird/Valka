import { useState } from "react";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { errorMessage } from "@/api/client";
import { useTasks, useClearAllTasks } from "@/hooks/use-tasks";
import { TaskFilters } from "@/components/tasks/task-filters";
import { TaskTable } from "@/components/tasks/task-table";
import { TaskCreateDialog } from "@/components/tasks/task-create-dialog";
import { Button } from "@/components/ui/button";
import { ErrorNote } from "@/components/ui/error-note";

const PAGE_SIZE = 25;

export function TasksPage() {
  const [filters, setFilters] = useState<{
    queue_name?: string;
    status?: string;
  }>({});
  const [offset, setOffset] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);
  const [cleared, setCleared] = useState<string | null>(null);

  // One more than a page: whether it comes back tells if there is a next page.
  const { data, isLoading, error, refetch } = useTasks({
    ...filters,
    limit: PAGE_SIZE + 1,
    offset,
  });
  const tasks = (data ?? []).slice(0, PAGE_SIZE);
  const hasMore = (data?.length ?? 0) > PAGE_SIZE;

  const clearAll = useClearAllTasks();

  function handleFilter(params: { queue_name?: string; status?: string }) {
    setFilters(params);
    setOffset(0);
  }

  function handleClearAll() {
    if (window.confirm("Delete ALL tasks, runs, logs, and dead letters? This cannot be undone.")) {
      setCleared(null);
      clearAll.mutate(undefined, {
        onSuccess: ({ deleted_count, missing }) => {
          setOffset(0);
          const deleted = `Deleted ${deleted_count.toLocaleString()} task${deleted_count === 1 ? "" : "s"}.`;
          setCleared(
            missing === null
              ? deleted
              : `${deleted} ${missing.length > 0 ? missing.join(", ") : "Some nodes"} did not answer: their tasks were not deleted.`,
          );
        },
      });
    }
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">Tasks</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Manage and monitor task execution
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={handleClearAll}
            disabled={clearAll.isPending}
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="mr-1.5 h-4 w-4" />
            Clear All
          </Button>
          <Button variant="outline" size="icon" aria-label="Refresh tasks" onClick={() => refetch()}>
            <RefreshCw className="h-4 w-4" />
          </Button>
          <Button onClick={() => setCreateOpen(true)}>
            <Plus className="h-4 w-4" />
            Create Task
          </Button>
        </div>
      </div>

      {clearAll.isError && (
        <ErrorNote compact>Could not clear tasks: {errorMessage(clearAll.error)}</ErrorNote>
      )}
      {cleared && !clearAll.isError && (
        <p
          role="status"
          className="rounded-md border border-primary/30 bg-primary/10 px-3 py-2 text-xs text-foreground"
        >
          {cleared}
        </p>
      )}

      <TaskFilters
        onFilter={handleFilter}
        initialQueue={filters.queue_name}
        initialStatus={filters.status}
      />

      {error && !data ? (
        <ErrorNote>Could not load tasks: {errorMessage(error)}</ErrorNote>
      ) : (
        <>
          {error && (
            <ErrorNote compact>
              Could not refresh tasks, showing the last answer: {errorMessage(error)}
            </ErrorNote>
          )}
          <TaskTable
            tasks={tasks}
            isLoading={isLoading}
            offset={offset}
            limit={PAGE_SIZE}
            hasMore={hasMore}
            onPageChange={setOffset}
          />
        </>
      )}

      <TaskCreateDialog open={createOpen} onClose={() => setCreateOpen(false)} />
    </div>
  );
}
