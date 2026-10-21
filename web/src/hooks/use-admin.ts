import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { adminApi } from "@/api/admin";

export function useQueues() {
  return useQuery({ queryKey: ["queues"], queryFn: adminApi.queues, refetchInterval: 10_000 });
}

/** The admin audit log; only fetched once an admin token is set. */
export function useAudit(enabled: boolean) {
  return useQuery({
    queryKey: ["admin", "audit"],
    queryFn: () => adminApi.audit(50),
    enabled,
    refetchInterval: 10_000,
  });
}

/** An admin action; refreshes the cluster views and the audit log when it succeeds. */
export function useAdminAction<A, R>(action: (args: A) => Promise<R>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: action,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cluster"] });
      queryClient.invalidateQueries({ queryKey: ["queues"] });
      queryClient.invalidateQueries({ queryKey: ["admin"] });
    },
  });
}
