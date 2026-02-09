import { useState, useEffect } from "react";
import { useParams, Link, useNavigate } from "react-router-dom";
import { ArrowLeft, Trash2 } from "lucide-react";
import { useTask, useTaskRuns, useDeleteTask } from "@/hooks/use-tasks";
import { TaskDetailPanel } from "@/components/task-detail/task-detail-panel";
import { TaskRunsTable } from "@/components/task-detail/task-runs-table";
import { TaskLogsViewer } from "@/components/task-detail/task-logs-viewer";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";

export function TaskDetailPage() {
  const { taskId } = useParams<{ taskId: string }>();
  const navigate = useNavigate();
  const { data: task, isLoading: taskLoading } = useTask(taskId!);
  const { data: runs = [], isLoading: runsLoading } = useTaskRuns(taskId!);
  const deleteTask = useDeleteTask();
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);

  // Auto-select the latest run when runs load
  useEffect(() => {
    if (runs.length > 0 && !selectedRunId) {
      setSelectedRunId(runs[0].id);
    }
  }, [runs, selectedRunId]);

  if (taskLoading) {
    return (
      <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
        Loading task...
      </div>
    );
  }

  if (!task) {
    return (
      <div className="flex h-64 items-center justify-center">
        <div className="text-center">
          <p className="text-sm text-muted-foreground">Task not found</p>
          <Button variant="link" asChild className="mt-2">
            <Link to="/tasks">
              <ArrowLeft className="h-4 w-4" />
              Back to tasks
            </Link>
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <Link
          to="/tasks"
          className="inline-flex items-center gap-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="h-4 w-4" />
          Back to tasks
        </Link>
        <Button
          variant="outline"
          size="sm"
          className="text-destructive hover:text-destructive"
          disabled={deleteTask.isPending}
          onClick={() => {
            if (window.confirm("Delete this task and all its runs/logs?")) {
              deleteTask.mutate(taskId!, {
                onSuccess: () => navigate("/tasks"),
              });
            }
          }}
        >
          <Trash2 className="mr-1.5 h-4 w-4" />
          Delete
        </Button>
      </div>

      <TaskDetailPanel task={task} />

      <Separator />

      <div className="space-y-4">
        <h3 className="text-lg font-semibold text-foreground">Runs</h3>
        <TaskRunsTable
          runs={runs}
          isLoading={runsLoading}
          selectedRunId={selectedRunId}
          onSelectRun={setSelectedRunId}
        />
      </div>

      {selectedRunId && (
        <div className="space-y-4">
          <h3 className="text-lg font-semibold text-foreground">Logs</h3>
          <TaskLogsViewer taskId={taskId!} runId={selectedRunId} />
        </div>
      )}
    </div>
  );
}
