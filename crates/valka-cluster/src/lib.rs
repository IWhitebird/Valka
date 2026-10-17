//! Node-to-node plumbing: the internal RPC vocabulary, clients for other nodes, and the
//! internal listener's security. Membership, placement and ownership live in
//! `valka-engine` (the bucket is the authority); this crate only moves requests.

pub mod peer;
pub mod rpc;
pub mod security;

pub use peer::PeerClients;
pub use rpc::{Command, Query, Reply, RpcError, ScatterReply, WorkerInfo};
