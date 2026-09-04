import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import type { ClusterNode, ClusterOverview } from "@/api/types";
import { HealthBanner } from "../health-banner";
import { NodeCard } from "../node-card";
import { WalPanel } from "../wal-panel";

function node(overrides: Partial<ClusterNode> = {}): ClusterNode {
  return {
    node_id: "node-a",
    epoch: 3,
    status: "alive",
    grpc_addr: "0.0.0.0:50051",
    http_addr: "0.0.0.0:8989",
    version: "0.1.0",
    started_at: new Date(Date.now() - 90_000).toISOString(),
    storage_backend: "s3",
    shards_owned: 4096,
    shards_with_tasks: 12,
    tasks: { pending: 5, running: 2, retry: 1, completed: 40, failed: 0, dead_letter: 1, cancelled: 0, total: 49 },
    queues: ["emails"],
    workers_connected: 3,
    wal: { epoch: 3, durable_lsn: "3:120", next_lsn: "3:121", unflushed_records: 0, oldest_unacked_ms: null, poisoned: null },
    snapshots: { last_round_at: null, dirty_shards: 4, oldest_dirty_lsn: "3:100", shards_with_snapshot: 40 },
    ...overrides,
  };
}

function overview(nodes: ClusterNode[], health?: Partial<ClusterOverview["health"]>): ClusterOverview {
  return {
    cluster_id: "valka",
    this_node: nodes[0]?.node_id ?? "",
    clustered: false,
    num_shards: 4096,
    health: { status: "ok", unowned_shards: 0, poisoned_nodes: [], suspect_nodes: [], ...health },
    nodes,
  };
}

const wrap = (ui: React.ReactElement) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe("HealthBanner", () => {
  it("summarises a healthy single node", () => {
    wrap(<HealthBanner overview={overview([node()])} />);
    expect(screen.getByText("Healthy")).toBeInTheDocument();
    expect(screen.getByText(/1\/1 node alive/)).toBeInTheDocument();
    expect(screen.getByText(/4,096\/4,096 shards owned/)).toBeInTheDocument();
    expect(screen.getByText(/WAL current/)).toBeInTheDocument();
    expect(screen.getByText(/Single-node mode/)).toBeInTheDocument();
  });

  it("lists problems when critical", () => {
    const n = node({ status: "poisoned", wal: { ...node().wal, poisoned: "bucket unreachable", unflushed_records: 17 } });
    wrap(<HealthBanner overview={overview([n], { status: "critical", poisoned_nodes: ["node-a"] })} />);
    expect(screen.getByText("Critical")).toBeInTheDocument();
    expect(screen.getByText(/WAL writer poisoned on node-a/)).toBeInTheDocument();
    expect(screen.getByText(/17 records awaiting commit/)).toBeInTheDocument();
  });
});

describe("NodeCard", () => {
  it("shows the key figures and links to the node page", () => {
    wrap(<NodeCard node={node()} numShards={4096} />);
    expect(screen.getByText("node-a")).toBeInTheDocument();
    expect(screen.getByText("epoch 3")).toBeInTheDocument();
    expect(screen.getByText("5 / 2")).toBeInTheDocument();
    expect(screen.getByText("3:120")).toBeInTheDocument();
    expect(screen.getByRole("link")).toHaveAttribute("href", "/cluster/nodes/node-a");
  });

  it("surfaces a poisoned writer", () => {
    wrap(<NodeCard node={node({ status: "poisoned", wal: { ...node().wal, poisoned: "segment PUT failed" } })} numShards={4096} />);
    expect(screen.getByText(/Writer poisoned: segment PUT failed/)).toBeInTheDocument();
  });
});

describe("WalPanel", () => {
  it("renders LSNs and a healthy writer", () => {
    wrap(<WalPanel node={node()} />);
    expect(screen.getByText("3:120")).toBeInTheDocument();
    expect(screen.getByText("3:121")).toBeInTheDocument();
    expect(screen.getByText("healthy")).toBeInTheDocument();
  });
  it("formats the oldest unacknowledged write", () => {
    wrap(<WalPanel node={node({ wal: { ...node().wal, unflushed_records: 9, oldest_unacked_ms: 4500 } })} />);
    expect(screen.getByText("4.5 s")).toBeInTheDocument();
  });
});
