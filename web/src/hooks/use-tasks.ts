import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { tasksApi } from "@/api/tasks";
import {
  EMPTY_LOG_WINDOW,
  appendLogs,
  earlierRequest,
  logsSettled,
  prependLogs,
  readNewLogs,
  type LogWindow,
} from "@/lib/logs";
import type {
  ListTasksParams,
  CreateTaskRequest,
  SendSignalRequest,
  ListDeadLettersParams,
} from "@/api/types";

export function useTasks(params: ListTasksParams = {}) {
  return useQuery({
    queryKey: ["tasks", params],
    queryFn: () => tasksApi.list(params),
  });
}

export function useTask(taskId: string) {
  return useQuery({
    queryKey: ["tasks", taskId],
    queryFn: () => tasksApi.get(taskId),
    enabled: !!taskId,
  });
}

export function useCreateTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: CreateTaskRequest) => tasksApi.create(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
    },
  });
}

export function useCancelTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (taskId: string) => tasksApi.cancel(taskId),
    onSuccess: (_data, taskId) => {
      queryClient.invalidateQueries({ queryKey: ["tasks", taskId] });
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
    },
  });
}

export function useDeleteTask() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (taskId: string) => tasksApi.delete(taskId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
      // A deleted dead-lettered task takes its dead letter with it.
      queryClient.invalidateQueries({ queryKey: ["dead-letters"] });
    },
  });
}

export function useClearAllTasks() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: () => tasksApi.clearAll(),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tasks"] });
      queryClient.invalidateQueries({ queryKey: ["dead-letters"] });
    },
  });
}

export function useTaskRuns(taskId: string) {
  return useQuery({
    queryKey: ["tasks", taskId, "runs"],
    queryFn: () => tasksApi.getRuns(taskId),
    enabled: !!taskId,
  });
}

export function useTaskCheckpoints(taskId: string) {
  return useQuery({
    queryKey: ["tasks", taskId, "checkpoints"],
    queryFn: () => tasksApi.getCheckpoints(taskId),
    enabled: !!taskId,
    refetchInterval: 5_000,
  });
}

function runLogsKey(taskId: string, runId: string) {
  return ["tasks", taskId, "runs", runId, "logs"];
}

/**
 * A run's log, newest lines kept. The first load catches up with the whole log; refreshes ask
 * only for lines after the newest one held, every 5 s until the run is over and a refresh
 * finds nothing new.
 */
export function useTaskRunLogs(taskId: string, runId: string, runFinished: boolean) {
  const queryClient = useQueryClient();
  return useQuery({
    queryKey: runLogsKey(taskId, runId),
    queryFn: async () => {
      const key = runLogsKey(taskId, runId);
      const start = queryClient.getQueryData<LogWindow>(key) ?? EMPTY_LOG_WINDOW;
      const read = await readNewLogs(start, (afterId, limit) =>
        tasksApi.getRunLogs(taskId, runId, { afterId, limit }),
      );
      // "Load earlier" may have changed the window meanwhile: add the new lines to that.
      const latest = queryClient.getQueryData<LogWindow>(key);
      if (latest && latest !== start) {
        return { ...appendLogs(latest, read.lines), caughtUp: read.caughtUp, added: read.added };
      }
      return read;
    },
    enabled: !!taskId && !!runId,
    refetchInterval: (query) => (logsSettled(query.state.data, runFinished) ? false : 5_000),
  });
}

/** Load the lines just before the first one shown. */
export function useLoadEarlierLogs(taskId: string, runId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const key = runLogsKey(taskId, runId);
      const current = queryClient.getQueryData<LogWindow>(key);
      const request = current ? earlierRequest(current) : null;
      if (!request) return;
      const lines = await tasksApi.getRunLogs(taskId, runId, request);
      queryClient.setQueryData<LogWindow>(key, (w) => (w ? prependLogs(w, lines) : w));
    },
  });
}

export function useTaskSignals(taskId: string) {
  return useQuery({
    queryKey: ["tasks", taskId, "signals"],
    queryFn: () => tasksApi.listSignals(taskId),
    enabled: !!taskId,
    refetchInterval: 5_000,
  });
}

export function useSendSignal() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ taskId, request }: { taskId: string; request: SendSignalRequest }) =>
      tasksApi.sendSignal(taskId, request),
    onSuccess: (_data, { taskId }) => {
      queryClient.invalidateQueries({ queryKey: ["tasks", taskId, "signals"] });
    },
  });
}

export function useDeadLetters(params: ListDeadLettersParams = {}) {
  return useQuery({
    queryKey: ["dead-letters", params],
    queryFn: () => tasksApi.listDeadLetters(params),
  });
}
