import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { clusterApi } from "@/api/cluster";
import type { ListShardsParams } from "@/api/types";

export function useClusterOverview() {
  return useQuery({
    queryKey: ["cluster"],
    queryFn: clusterApi.overview,
    refetchInterval: 5_000,
  });
}

export function useShards(params: ListShardsParams = {}) {
  return useQuery({
    queryKey: ["cluster", "shards", params],
    queryFn: () => clusterApi.shards(params),
    refetchInterval: 5_000,
  });
}

export function useShard(shard: number | null) {
  return useQuery({
    queryKey: ["cluster", "shard", shard],
    queryFn: () => clusterApi.shard(shard as number),
    enabled: shard !== null,
    refetchInterval: 5_000,
  });
}

export function useStorage() {
  return useQuery({
    queryKey: ["cluster", "storage"],
    queryFn: clusterApi.storage,
    refetchInterval: 30_000,
  });
}

export function useSnapshotNow() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: clusterApi.snapshotNow,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["cluster"] });
    },
  });
}
