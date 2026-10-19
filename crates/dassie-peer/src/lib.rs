//! Dassie's peer protocol v1 (spec/areas/peer-protocol.md), as sans-IO code.
//!
//! The protocol is the original's (spec/original/03-peer-network.md),
//! versioned: the v0 message set and layouts, with sender authentication,
//! replay protection, authenticated responses and the membership and
//! peering fixes of D2, D4 and D12. This crate holds its wire format; the
//! state machines that use it are built on top.
//!
//! Like the other protocol crates, it never reads a clock, an RNG or a
//! socket of its own (D16).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod auth;
mod channel;
mod config;
mod hooks;
mod io;
mod node;
mod persist;
pub mod routing;
#[cfg(feature = "sim")]
pub mod sim;
mod table;
pub mod wire;

pub use channel::{Channel, Outgoing};
pub use config::{Config, Rate};
pub use hooks::{Agreed, Declined, PeeringHooks};
pub use io::{
    Event, Failure, Inbound, InboundId, Output, Reply, Request, RequestId, Response,
    SettlementAnswer, Stats, VERDICT_ACCEPTED,
};
pub use node::{
    BanError, Bootstrap, NodeView, OwnInfo, PeerNode, PeerStatus, RegisterError,
    RegistrationStatus, SendError,
};
pub use persist::{PeerBatch, Stored, StoredChannel, StoredNode};
pub use table::{Advertise, Network, Peering, Registration, Room, Sources, Termination};
