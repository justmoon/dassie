//! Dassie's peer protocol v1 (spec/areas/peer-protocol.md), as sans-IO code.
//!
//! The protocol is the original's, versioned: the v0 message set and
//! layouts, carried over QUIC, whose connection proves the sender's node
//! key, with the membership and peering fixes of
//! spec/decisions/peer-protocol-v1.md,
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
    Response, SettlementAnswer, Stats,
};
pub use node::{
    BanError, Bootstrap, MAX_METHODS, MAX_OWN_HUBS, MAX_SCHEMES, MAX_UPLINKS, NodeView, OwnInfo,
    PeerNode, PeerStatus, RegisterError, RegistrationStatus, SendError,
};
pub use persist::{PeerBatch, Stored, StoredChannel, StoredClosed, StoredNode};
pub use table::{
    Advertise, Depeering, Method, Network, Peering, Registration, Room, Sources, Termination,
};
