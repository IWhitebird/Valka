import { fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ClusterOperationsPage } from "../cluster-operations";
import { credentials } from "@/lib/credentials";

const overview = {
  cluster_id: "c",
  this_node: "n1",
  clustered: true,
  num_shards: 256,
  health: { status: "ok", unowned_shards: 0, poisoned_nodes: [], suspect_nodes: [] },
  nodes: [
    {
      node_id: "n1",
      inc: 2,
      status: "alive",
      grpc_addr: null,
      http_addr: null,
      version: "0.2.0",
      started_at: null,
      lease_age_ms: 100,
      eligible: true,
      storage_backend: "s3",
      shards_owned: 256,
      shards_with_tasks: 3,
      tasks: { pending: 0, running: 0, retry: 0, completed: 0, failed: 0, dead_letter: 0, cancelled: 0, total: 0 },
      queues: [],
      workers_connected: 1,
      wal: { inc: 2, durable_lsn: "2:1", next_lsn: "2:2", unflushed_records: 0, oldest_unacked_ms: null, poisoned: null },
      snapshots: { last_round_at: null, dirty_shards: 0, oldest_dirty_lsn: null, shards_with_snapshot: 0 },
    },
  ],
};

function api(path: string): unknown {
  if (path.startsWith("/api/v1/cluster/audit")) return [];
  if (path.startsWith("/api/v1/cluster/placement")) return { pins: { "7": "n1" }, effective_protocol: 2, eligible_nodes: ["n1"] };
  if (path.startsWith("/api/v1/queues")) return [{ queue: "emails", partitions: 4, served_partitions: 16, previous: null, version: 2, created_at: "", partition_shards: [] }];
  if (path.startsWith("/api/v1/cluster")) return overview;
  return [];
}

afterEach(() => {
  vi.unstubAllGlobals();
  credentials.setAdminToken(null);
});

describe("ClusterOperationsPage", () => {
  it("lists nodes, pins and queues, with actions locked until the admin token is set", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string) => new Response(JSON.stringify(api(path)), { status: 200 })),
    );
    render(
      <QueryClientProvider client={new QueryClient()}>
        <MemoryRouter>
          <ClusterOperationsPage />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    expect(await screen.findByText("emails")).toBeInTheDocument();
    expect(await screen.findByText("7 → n1")).toBeInTheDocument();
    expect(screen.getByText("12")).toBeInTheDocument(); // 16 served - 4 current: draining
    expect(screen.getByLabelText("Admin token")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Drain/ })).toBeDisabled();
    expect(screen.getByText("Set the admin token to see who changed what.")).toBeInTheDocument();
  });

  it("does not carry a failed drain over to the next node's dialog", async () => {
    credentials.setAdminToken("admin");
    vi.stubGlobal(
      "fetch",
      vi.fn(async (path: string, init?: RequestInit) => {
        if (path.endsWith("/drain") && init?.method === "POST") {
          return new Response(JSON.stringify({ error: "n1 did not accept the drain", code: "UNAVAILABLE" }), {
            status: 503,
          });
        }
        return new Response(JSON.stringify(api(path)), { status: 200 });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter>
          <ClusterOperationsPage />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    fireEvent.click(await screen.findByRole("button", { name: /Drain/ }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Drain node" }));
    expect(await within(dialog).findByText("n1 did not accept the drain")).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole("button", { name: "Keep it running" }));
    fireEvent.click(screen.getByRole("button", { name: /Drain/ }));
    const reopened = await screen.findByRole("dialog");
    expect(within(reopened).queryByText("n1 did not accept the drain")).not.toBeInTheDocument();
  });
});
