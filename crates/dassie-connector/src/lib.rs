//! Dassie's ILP connector (spec/areas/connector.md), as a sans-IO state
//! machine.
//!
//! The [`Connector`] knows the node's endpoints and their accounts, routes
//! each Prepare to one of them, answers ILDCP, books the packet's pending
//! transfers in the [`dassie_ledger::Ledger`], and resolves them when the
//! response comes back or the packet expires. Every packet gets a
//! [`TraceId`](dassie_ledger::TraceId) that its transfers carry, and a
//! record in a bounded trace, so the journal and the packets line up.
//!
//! The behaviour is ILPv4's, fixing the original's gaps
//! (spec/decisions/connector-timing.md):
//!
//! - packets time out at their outgoing expiry, not after a fixed 5 s;
//! - an invalid fulfillment is rejected with F05 at once, except
//!   a peer's, which may be a resend of an earlier packet's;
//! - malformed packets get F01 and unroutable ones F02; nothing panics on
//!   the packet path;
//! - F08 carries the amounts;
//! - linked transfers are all or none (through the ledger);
//! - the owner's own endpoints never convert, and only transit from one
//!   peer to another is charged a fee and a spread
//!   (spec/decisions/exchange-rates.md), and priced by the owner's
//!   inventory (spec/decisions/passive-rebalancing.md);
//! - ILDCP answers the address that is routed and the endpoint's asset;
//! - outgoing request IDs are unique per endpoint;
//! - prefixes match whole segments.
//!
//! It never reads a clock: `now` comes in with every call, and the caller
//! polls [`Connector::poll_timeout`] for the next deadline
//! (spec/decisions/tigerstyle.md). Packets are written into a buffer the
//! caller passes, so the packet path does not allocate.

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
pub mod pricing;
#[cfg(feature = "sim")]
pub mod sim;
mod trace;

pub use cap::{CapError, CapUsage, CapView, SpendingCap, replay};
pub use charges::Charges;
pub use config::Config;
pub use connector::{
    Connector, Forwarded, HubWait, MAX_CHILD_ADDRESSES, MAX_ROUTE_OPTIONS, OpensAccounts,
    OpensNone, Outcome, Posted, Responded, RoutesError, SWAP_PREFIX, Stats, Upstream, swap_scheme,
};
pub use endpoint::{EndpointError, EndpointId, MAX_SLOTS, SlotError, SlotView};
pub use fx::{RATE_PRECISION, Rates, convert, convert_at};
pub use pricing::{
    BestOutput, Holding, Holdings, Inventory, Price, PriceError, PriceTable, Pricing,
};
pub use trace::{MAX_TRACED_ADDRESS, PacketRecord, PacketState, TraceEvent};

/// A packet's pending transfers: the first debits the source's account;
/// the second, across ledgers, credits the destination's from the FX
/// accounts; the third is our fee on transit, which debits the source's
/// account too.
pub type PacketTransfers = [Option<dassie_ledger::TransferId>; 3];
