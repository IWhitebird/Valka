import { useState } from "react";
import { useParams, Link, useNavigate } from "react-router-dom";
import { ArrowLeft, Trash2 } from "lucide-react";
import { errorMessage, isNotFound } from "@/api/client";
import { useTask, useTaskRuns, useTaskCheckpoints, useDeleteTask } from "@/hooks/use-tasks";
import { TaskDetailPanel } from "@/components/task-detail/task-detail-panel";
import { TaskSignalsPanel } from "@/components/task-detail/task-signals-panel";
import { TaskRunsTable } from "@/components/task-detail/task-runs-table";
import { TaskCheckpointsTable } from "@/components/task-detail/task-checkpoints-table";
import { TaskLogsViewer } from "@/components/task-detail/task-logs-viewer";
import { Button } from "@/components/ui/button";
import { ErrorNote } from "@/components/ui/error-note";
import { Separator } from "@/components/ui/separator";

function BackToTasks() {
  return (
    <Button variant="link" asChild className="mt-2">
      <Link to="/tasks">
        <ArrowLeft className="h-4 w-4" />
        Back to tasks
      </Link>
    </Button>
  );
}

export function TaskDetailPage() {
  const { taskId } = useParams<{ taskId: string }>();
  const navigate = useNavigate();
  const { data: task, isLoading: taskLoading, error: taskError } = useTask(taskId!);
  const {
    data: runs = [],
    isLoading: runsLoading,
    error: runsError,
  } = useTaskRuns(taskId!);
  const { data: checkpoints = [], error: checkpointsError } = useTaskCheckpoints(taskId!);
  const deleteTask = useDeleteTask();
  const [chosenRunId, setSelectedRunId] = useState<string | null>(null);
  // The run the user picked, else the latest one.
  const selectedRunId = chosenRunId ?? runs[0]?.id ?? null;
  const selectedRun = runs.find((r) => r.id === selectedRunId);

  if (taskLoading) {
    return (
      <div className="flex h-64 items-center justify-center text-sm text-muted-foreground">
        Loading task...
      </div>
    );
  }

  if (!task) {
    // Only a 404 means the task does not exist; anything else is a failure to say so.
    if (taskError && !isNotFound(taskError)) {
      return (
        <div className="space-y-2">
          <ErrorNote>Could not load task: {errorMessage(taskError)}</ErrorNote>
          <BackToTasks />
        </div>
      );
    }
    return (
      <div className="flex h-64 items-center justify-center">
        <div className="text-center">
          <p className="text-sm text-muted-foreground">Task not found</p>
          <BackToTasks />
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

      {deleteTask.isError && (
        <ErrorNote compact>Could not delete the task: {errorMessage(deleteTask.error)}</ErrorNote>
      )}

      {taskError && (
        <ErrorNote compact>
          Could not refresh the task, showing the last known state: {errorMessage(taskError)}
        </ErrorNote>
      )}

      <TaskDetailPanel task={task} />

      <Separator />

      <TaskSignalsPanel taskId={taskId!} taskStatus={task.status} />

      <Separator />

      {checkpointsError && !isNotFound(checkpointsError) && (
        <ErrorNote compact>Could not load steps: {errorMessage(checkpointsError)}</ErrorNote>
      )}

      {checkpoints.length > 0 && (
        <div className="space-y-4">
          <h3 className="text-lg font-semibold text-foreground">Steps</h3>
          <TaskCheckpointsTable checkpoints={checkpoints} />
        </div>
      )}

      <div className="space-y-4">
        <h3 className="text-lg font-semibold text-foreground">Runs</h3>
        {runsError && runs.length === 0 ? (
          <ErrorNote>Could not load runs: {errorMessage(runsError)}</ErrorNote>
        ) : (
          <TaskRunsTable
            runs={runs}
            isLoading={runsLoading}
            selectedRunId={selectedRunId}
            onSelectRun={setSelectedRunId}
          />
        )}
      </div>

      {selectedRunId && (
        <div className="space-y-4">
          <h3 className="text-lg font-semibold text-foreground">Logs</h3>
          <TaskLogsViewer
            taskId={taskId!}
            runId={selectedRunId}
            runFinished={selectedRun ? selectedRun.status !== "RUNNING" : true}
          />
        </div>
      )}
    </div>
  );
}
