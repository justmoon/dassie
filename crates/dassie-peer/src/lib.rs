//! Dassie's peer protocol v1 (spec 7 §4.7), as sans-IO code.
//!
//! The protocol is spec 3's, versioned: the v0 message set and layouts,
//! with sender authentication, replay protection, authenticated responses
//! and the membership and peering fixes of D2, D4 and D12. This crate holds
//! its wire format; the state machines that use it are built on top.
//!
//! Like the other protocol crates, it never reads a clock, an RNG or a
//! socket of its own (D16).

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    clippy::too_many_lines
)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod auth;
mod config;
mod hooks;
mod io;
mod node;
mod persist;
pub mod routing;
mod table;
pub mod wire;

pub use config::{Config, Rate};
pub use hooks::{Agreed, Declined, PeeringHooks};
pub use io::{
    Event, Failure, Inbound, InboundId, Output, Reply, Request, RequestId, Response, Stats,
};
pub use node::{Bootstrap, NodeView, OwnInfo, PeerNode, RegistrationStatus, SendError};
pub use persist::{PeerBatch, Stored, StoredNode};
pub use table::{Peering, Registration, Sources};
