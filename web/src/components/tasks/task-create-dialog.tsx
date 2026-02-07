import { useState } from "react";
import { useCreateTask } from "@/hooks/use-tasks";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Button } from "@/components/ui/button";

interface TaskCreateDialogProps {
  open: boolean;
  onClose: () => void;
}

export function TaskCreateDialog({ open, onClose }: TaskCreateDialogProps) {
  const [queueName, setQueueName] = useState("");
  const [taskName, setTaskName] = useState("");
  const [input, setInput] = useState("");
  const [priority, setPriority] = useState(0);
  const [maxRetries, setMaxRetries] = useState(3);
  const [timeoutSeconds, setTimeoutSeconds] = useState(300);
  const [scheduledAt, setScheduledAt] = useState("");
  const [idempotencyKey, setIdempotencyKey] = useState("");
  const [metadata, setMetadata] = useState("");
  const [error, setError] = useState<string | null>(null);

  const createTask = useCreateTask();

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);

    let parsedInput: Record<string, unknown> | undefined;
    if (input.trim()) {
      try {
        parsedInput = JSON.parse(input);
      } catch {
        setError("Invalid JSON in input field");
        return;
      }
    }

    let parsedMetadata: Record<string, unknown> | undefined;
    if (metadata.trim()) {
      try {
        parsedMetadata = JSON.parse(metadata);
      } catch {
        setError("Invalid JSON in metadata field");
        return;
      }
    }

    createTask.mutate(
      {
        queue_name: queueName,
        task_name: taskName,
        input: parsedInput,
        priority,
        max_retries: maxRetries,
        timeout_seconds: timeoutSeconds,
        scheduled_at: scheduledAt || undefined,
        idempotency_key: idempotencyKey || undefined,
        metadata: parsedMetadata,
      },
      {
        onSuccess: () => {
          resetForm();
          onClose();
        },
        onError: (err) => {
          setError(err.message);
        },
      },
    );
  }

  function resetForm() {
    setQueueName("");
    setTaskName("");
    setInput("");
    setPriority(0);
    setMaxRetries(3);
    setTimeoutSeconds(300);
    setScheduledAt("");
    setIdempotencyKey("");
    setMetadata("");
    setError(null);
  }

  return (
    <Dialog open={open} onOpenChange={(isOpen) => !isOpen && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Create Task</DialogTitle>
          <DialogDescription>
            Create a new task to be dispatched to an available worker.
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={handleSubmit} className="space-y-4">
          {error && (
            <div className="rounded-md border border-destructive/30 bg-destructive/10 px-4 py-2.5 text-sm text-destructive">
              {error}
            </div>
          )}

          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-2">
              <Label htmlFor="queue_name">Queue Name *</Label>
              <Input
                id="queue_name"
                type="text"
                required
                value={queueName}
                onChange={(e) => setQueueName(e.target.value)}
                placeholder="default"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="task_name">Task Name *</Label>
              <Input
                id="task_name"
                type="text"
                required
                value={taskName}
                onChange={(e) => setTaskName(e.target.value)}
                placeholder="send_email"
              />
            </div>
          </div>

          <div className="space-y-2">
            <Label htmlFor="input_json">Input (JSON)</Label>
            <Textarea
              id="input_json"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder='{"key": "value"}'
              rows={3}
              className="font-mono"
            />
          </div>

          <div className="grid grid-cols-3 gap-4">
            <div className="space-y-2">
              <Label htmlFor="priority">Priority</Label>
              <Input
                id="priority"
                type="number"
                value={priority}
                onChange={(e) => setPriority(Number(e.target.value))}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="max_retries">Max Retries</Label>
              <Input
                id="max_retries"
                type="number"
                value={maxRetries}
                onChange={(e) => setMaxRetries(Number(e.target.value))}
                min={0}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="timeout_seconds">Timeout (s)</Label>
              <Input
                id="timeout_seconds"
                type="number"
                value={timeoutSeconds}
                onChange={(e) => setTimeoutSeconds(Number(e.target.value))}
                min={1}
              />
            </div>
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div className="space-y-2">
              <Label htmlFor="scheduled_at">Schedule At (ISO 8601)</Label>
              <Input
                id="scheduled_at"
                type="datetime-local"
                value={scheduledAt}
                onChange={(e) => setScheduledAt(e.target.value ? new Date(e.target.value).toISOString() : "")}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="idempotency_key">Idempotency Key</Label>
              <Input
                id="idempotency_key"
                type="text"
                value={idempotencyKey}
                onChange={(e) => setIdempotencyKey(e.target.value)}
                placeholder="unique-key-123"
              />
            </div>
          </div>

          <div className="space-y-2">
            <Label htmlFor="metadata_json">Metadata (JSON)</Label>
            <Textarea
              id="metadata_json"
              value={metadata}
              onChange={(e) => setMetadata(e.target.value)}
              placeholder='{"source": "web-ui"}'
              rows={2}
              className="font-mono"
            />
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={createTask.isPending}>
              {createTask.isPending ? "Creating..." : "Create Task"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
