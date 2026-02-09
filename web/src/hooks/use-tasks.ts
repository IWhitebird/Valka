import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { tasksApi } from "@/api/tasks";
import type {
  ListTasksParams,
  CreateTaskRequest,
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

export function useTaskRunLogs(taskId: string, runId: string) {
  return useQuery({
    queryKey: ["tasks", taskId, "runs", runId, "logs"],
    queryFn: () => tasksApi.getRunLogs(taskId, runId),
    enabled: !!taskId && !!runId,
    refetchInterval: 5_000,
  });
}

export function useDeadLetters(params: ListDeadLettersParams = {}) {
  return useQuery({
    queryKey: ["dead-letters", params],
    queryFn: () => tasksApi.listDeadLetters(params),
  });
}
