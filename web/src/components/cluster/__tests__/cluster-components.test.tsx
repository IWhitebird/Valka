import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it } from "vitest";
import type {
  ClusterMember,
  ClusterNode,
  ClusterNodeLease,
  ClusterNodeStats,
  ClusterOverview,
} from "@/api/types";
import { HealthBanner } from "../health-banner";
import { MembersTable } from "../members-table";
import { NodeCard } from "../node-card";
import { WalPanel } from "../wal-panel";

function node(overrides: Partial<ClusterNodeStats> = {}): ClusterNodeStats {
  return {
    node_id: "node-a",
    inc: 3,
    status: "alive",
    lease_age_ms: 1200,
    eligible: true,
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
    wal: { inc: 3, durable_lsn: "3:120", next_lsn: "3:121", unflushed_records: 0, oldest_unacked_ms: null, poisoned: null },
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
  it("counts shards only on nodes that answered", () => {
    const nodes: ClusterNode[] = [node({ shards_owned: 4096 }), leaseOnly()];
    wrap(<HealthBanner overview={{ ...overview(nodes), clustered: true }} />);
    expect(screen.getByText(/1\/2 nodes alive/)).toBeInTheDocument();
    expect(screen.getByText(/4,096\/4,096 shards owned/)).toBeInTheDocument();
    expect(screen.queryByText(/Single-node mode/)).not.toBeInTheDocument();
  });

  it("flags unknown ownership when a node did not answer", () => {
    wrap(
      <HealthBanner
        overview={overview([node()], { status: "degraded", unowned_shards: null, partial: true })}
      />,
    );
    expect(screen.getByText(/shard ownership is unknown/)).toBeInTheDocument();
  });

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

function leaseOnly(overrides: Partial<ClusterNodeLease> = {}): ClusterNodeLease {
  return {
    node_id: "node-b",
    inc: 7,
    status: "dead",
    grpc_addr: null,
    http_addr: null,
    internal_addr: "node-b:50052",
    version: "0.1.0",
    started_at: null,
    lease_age_ms: 42_000,
    eligible: false,
    protocol: 2,
    unsplit: [],
    ...overrides,
  };
}

describe("NodeCard", () => {
  it("shows the key figures and links to the node page", () => {
    wrap(<NodeCard node={node()} numShards={4096} />);
    expect(screen.getByText("node-a")).toBeInTheDocument();
    expect(screen.getByText("inc 3")).toBeInTheDocument();
    expect(screen.getByText("5 / 2")).toBeInTheDocument();
    expect(screen.getByText("3:120")).toBeInTheDocument();
    expect(screen.getByRole("link")).toHaveAttribute("href", "/cluster/nodes/node-a");
  });

  it("shows a node known only from its lease", () => {
    wrap(<NodeCard node={leaseOnly()} numShards={4096} />);
    expect(screen.getByText("node-b")).toBeInTheDocument();
    expect(screen.getByText("inc 7")).toBeInTheDocument();
    expect(screen.getByText(/Known from its lease only: the node is gone/)).toBeInTheDocument();
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

function member(overrides: Partial<ClusterMember> = {}): ClusterMember {
  return {
    node_id: "node-a",
    inc: 3,
    state: "alive",
    status: "alive",
    live: true,
    suspect: false,
    eligible: true,
    lease_age_ms: 900,
    grpc_addr: null,
    http_addr: null,
    internal_addr: "node-a:50052",
    version: "0.1.0",
    protocol: 2,
    started_at: null,
    unsplit: [],
    ...overrides,
  };
}

describe("MembersTable", () => {
  it("lists every lease with its incarnation, state and placement", () => {
    wrap(
      <MembersTable
        members={[
          member(),
          member({ node_id: "node-b", inc: 5, state: "dead", status: "dead", live: false, eligible: false, lease_age_ms: 60_000, unsplit: [4] }),
        ]}
        placement={{ pins: { "17": "node-a" }, effective_protocol: 2, eligible_nodes: ["node-a"] }}
        thisNode="node-a"
      />,
    );
    expect(screen.getByText(/seen from node-a/)).toBeInTheDocument();
    expect(screen.getByText("node-b")).toBeInTheDocument();
    expect(screen.getByText("dead")).toBeInTheDocument();
    expect(screen.getByText("not eligible")).toBeInTheDocument();
    expect(screen.getByText("4")).toBeInTheDocument();
    expect(screen.getByText(/effective protocol 2 · 1 eligible/)).toBeInTheDocument();
    expect(screen.getByText("17→node-a")).toBeInTheDocument();
  });
});
