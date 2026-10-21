import { credentials } from "@/lib/credentials";
import { fetchAPI, pathSegment } from "./client";
import type { AuditRecord, QueueInfo } from "./types";

/** Admin calls carry the admin token instead of the API key. */
function asAdmin(init: RequestInit = {}): RequestInit {
  return {
    ...init,
    headers: { ...(init.headers ?? {}), Authorization: `Bearer ${credentials.adminToken() ?? ""}` },
  };
}

export const adminApi = {
  queues(): Promise<QueueInfo[]> {
    return fetchAPI<QueueInfo[]>("/api/v1/queues");
  },

  audit(limit = 50): Promise<AuditRecord[]> {
    return fetchAPI<AuditRecord[]>(`/api/v1/cluster/audit?limit=${limit}`, asAdmin());
  },

  drainNode(node: string): Promise<{ accepted: boolean }> {
    return fetchAPI(`/api/v1/cluster/nodes/${pathSegment(node)}/drain`, asAdmin({ method: "POST" }));
  },

  moveShard(shard: number, to: string): Promise<{ shard: number; pinned_to: string }> {
    return fetchAPI(
      `/api/v1/cluster/shards/${shard}/move`,
      asAdmin({ method: "POST", body: JSON.stringify({ to }) }),
    );
  },

  unpinShard(shard: number): Promise<{ shard: number }> {
    return fetchAPI(`/api/v1/cluster/shards/${shard}/pin`, asAdmin({ method: "DELETE" }));
  },

  setPartitions(
    queue: string,
    partitions: number,
  ): Promise<{ queue: string; partitions: number; previous: number | null; nodes_not_told: string[] }> {
    return fetchAPI(
      `/api/v1/queues/${pathSegment(queue)}/partitions`,
      asAdmin({ method: "PUT", body: JSON.stringify({ partitions }) }),
    );
  },
};
