//! Settlement schemes: how a node moves real (or test) money with its
//! peers on an underlying ledger, and how the books learn about it.
//!
//! - [`SettlementScheme`] is the interface of spec 5 §4.3, sans-IO: methods
//!   return [`Effect`]s for the node to book, rather than calling back into
//!   the node. The changes of spec 7 §4.5 are built in: settlements have
//!   explicit IDs, a failed one is cancelled, peer state returned by the
//!   module is kept, versions are declared for negotiation, and config is
//!   passed through.
//! - [`activation`] decides what to book when a scheme starts, from its
//!   balance and the last checkpoint, so that restarts book nothing twice
//!   (ACC-6).
//! - [`policy`] decides when to settle with a peer, and how much.
//! - [`Stub`] is the test-realm scheme without an underlying ledger.
//!
//! A scheme with an underlying ledger to follow, such as XRPL's, does no
//! IO of its own: it says what to send ([`Effect::LedgerSend`]) and the
//! node feeds it what comes back ([`LedgerInput`]). The node keeps the
//! registry of schemes, since they live in their own crates.

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

mod activation;
pub mod policy;
mod scheme;
mod stub;

pub use activation::{Activation, activation};
pub use scheme::{
    Effect, LedgerInput, LedgerStatus, PreparedSettlement, SchemeError, SchemeInfo,
    SettlementScheme,
};
pub use stub::{STUB_INITIAL_BALANCE, Stub};
