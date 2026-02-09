import { useState } from "react";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { useTasks, useClearAllTasks } from "@/hooks/use-tasks";
import { TaskFilters } from "@/components/tasks/task-filters";
import { TaskTable } from "@/components/tasks/task-table";
import { TaskCreateDialog } from "@/components/tasks/task-create-dialog";
import { Button } from "@/components/ui/button";

const PAGE_SIZE = 25;

export function TasksPage() {
  const [filters, setFilters] = useState<{
    queue_name?: string;
    status?: string;
  }>({});
  const [offset, setOffset] = useState(0);
  const [createOpen, setCreateOpen] = useState(false);

  const {
    data: tasks = [],
    isLoading,
    refetch,
  } = useTasks({
    ...filters,
    limit: PAGE_SIZE,
    offset,
  });

  const clearAll = useClearAllTasks();

  function handleFilter(params: { queue_name?: string; status?: string }) {
    setFilters(params);
    setOffset(0);
  }

  function handleClearAll() {
    if (window.confirm("Delete ALL tasks, runs, logs, and dead letters? This cannot be undone.")) {
      clearAll.mutate(undefined, {
        onSuccess: () => setOffset(0),
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
          <Button variant="outline" size="icon" onClick={() => refetch()}>
            <RefreshCw className="h-4 w-4" />
          </Button>
          <Button onClick={() => setCreateOpen(true)}>
            <Plus className="h-4 w-4" />
            Create Task
          </Button>
        </div>
      </div>

      <TaskFilters
        onFilter={handleFilter}
        initialQueue={filters.queue_name}
        initialStatus={filters.status}
      />

      <TaskTable
        tasks={tasks}
        isLoading={isLoading}
        offset={offset}
        limit={PAGE_SIZE}
        onPageChange={setOffset}
      />

      <TaskCreateDialog open={createOpen} onClose={() => setCreateOpen(false)} />
    </div>
  );
}
