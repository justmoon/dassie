//! Dassie's ILP connector (spec 4 §4), as a sans-IO state machine.
//!
//! The [`Connector`] knows the node's endpoints and their accounts, routes
//! each Prepare to one of them, answers ILDCP, books the packet's pending
//! transfers in the [`dassie_ledger::Ledger`], and resolves them when the
//! response comes back or the packet expires. Every packet gets a
//! [`TraceId`](dassie_ledger::TraceId) that its transfers carry, and a
//! record in a bounded trace, so the journal and the packets line up.
//!
//! The behaviour is ILPv4's, fixing the original's gaps (D13):
//!
//! - packets time out at their outgoing expiry, not after a fixed 5 s
//!   (ILP-1);
//! - an invalid fulfillment is rejected with F05 at once (ILP-2), except
//!   a peer's, which may be a resend of an earlier packet's;
//! - malformed packets get F01 and unroutable ones F02; nothing panics on
//!   the packet path (ILP-3);
//! - F08 carries the amounts (ILP-4);
//! - linked transfers are all or none (ILP-5, through the ledger);
//! - the owner's own endpoints never convert, and only transit from one
//!   peer to another is charged a fee and a spread (D7);
//! - ILDCP answers the address that is routed and the endpoint's asset
//!   (ILP-7, NET-25);
//! - outgoing request IDs are unique per endpoint (ILP-8);
//! - prefixes match whole segments (NET-33).
//!
//! It never reads a clock: `now` comes in with every call, and the caller
//! polls [`Connector::poll_timeout`] for the next deadline (D16). Packets
//! are written into a buffer the caller passes, so the packet path does not
//! allocate.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod cap;
mod charges;
mod config;
mod connector;
mod endpoint;
mod fx;
mod trace;

pub use cap::{CapError, CapUsage, CapView, SpendingCap, replay};
pub use charges::Charges;
pub use config::Config;
pub use connector::{
    Connector, Forwarded, MAX_ROUTE_OPTIONS, Outcome, Posted, Responded, RoutesError, Stats,
    Upstream,
};
pub use endpoint::{EndpointError, EndpointId};
pub use fx::{RATE_PRECISION, Rates, convert};
pub use trace::{MAX_TRACED_ADDRESS, PacketRecord, PacketState, TraceEvent};

/// A packet's pending transfers: the first debits the source's account;
/// the second, across ledgers, credits the destination's from the FX
/// accounts; the third is our fee on transit, which debits the source's
/// account too.
pub type PacketTransfers = [Option<dassie_ledger::TransferId>; 3];
