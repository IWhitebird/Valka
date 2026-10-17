import { fetchAPI } from "./client";
import type {
  ClusterMember,
  ClusterOverview,
  ClusterPlacement,
  ListShardsParams,
  ShardDetail,
  ShardStats,
  SnapshotStats,
  StorageStats,
} from "./types";

export const clusterApi = {
  overview(): Promise<ClusterOverview> {
    return fetchAPI<ClusterOverview>("/api/v1/cluster");
  },

  shards(params: ListShardsParams = {}): Promise<ShardStats[]> {
    const searchParams = new URLSearchParams();
    if (params.node) searchParams.set("node", params.node);
    if (params.dirty) searchParams.set("dirty", "true");
    if (params.min_tasks !== undefined)
      searchParams.set("min_tasks", String(params.min_tasks));
    const query = searchParams.toString();
    return fetchAPI<ShardStats[]>(
      `/api/v1/cluster/shards${query ? `?${query}` : ""}`,
    );
  },

  shard(shard: number): Promise<ShardDetail> {
    return fetchAPI<ShardDetail>(`/api/v1/cluster/shards/${shard}`);
  },

  members(): Promise<ClusterMember[]> {
    return fetchAPI<ClusterMember[]>("/api/v1/cluster/members");
  },

  placement(): Promise<ClusterPlacement> {
    return fetchAPI<ClusterPlacement>("/api/v1/cluster/placement");
  },

  storage(): Promise<StorageStats> {
    return fetchAPI<StorageStats>("/api/v1/cluster/storage");
  },

  snapshotNow(): Promise<{ snapshots: SnapshotStats; durable_lsn: string }> {
    return fetchAPI("/api/v1/cluster/snapshot", { method: "POST" });
  },
};
