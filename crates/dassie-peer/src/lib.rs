//! Dassie's peer protocol v1 (spec/areas/peer-protocol.md), as sans-IO code.
//!
//! The protocol is the original's (spec/original/03-peer-network.md),
//! versioned: the v0 message set and layouts, with sender authentication,
//! replay protection, authenticated responses and the membership and
//! peering fixes of spec/decisions/peer-protocol-v1.md,
//! spec/decisions/membership-economics.md and
//! spec/decisions/peering-lifecycle.md. This crate holds its wire format; the
//! state machines that use it are built on top.
//!
//! Like the other protocol crates, it never reads a clock, an RNG or a
//! socket of its own (spec/decisions/tigerstyle.md).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod auth;
mod channel;
mod config;
mod hooks;
mod io;
pub mod names;
mod node;
mod persist;
pub mod routing;
#[cfg(feature = "sim")]
pub mod sim;
mod table;
pub mod wire;

pub use channel::{Channel, Closed, Outgoing};
pub use config::{Config, Rate};
pub use hooks::{Agreed, Declined, EntryPayment, PeeringHooks};
pub use io::{
    Event, Failure, Inbound, InboundId, NameEvent, Output, PacketAnswer, Reply, Request, RequestId,
    Response, SettlementAnswer, Stats, VERDICT_ACCEPTED,
};
pub use node::{
    BanError, Bootstrap, MAX_SCHEMES, MAX_UPLINKS, NodeView, OwnInfo, PeerNode, PeerStatus,
    RegisterError, RegistrationStatus, SendError,
};
pub use persist::{PeerBatch, Stored, StoredChannel, StoredClosed, StoredFloor, StoredNode};
pub use table::{Advertise, Depeering, Network, Peering, Registration, Room, Sources, Termination};
