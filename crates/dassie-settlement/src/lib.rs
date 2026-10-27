//! Settlement schemes: how a node moves real (or test) money with its
//! peers on an underlying ledger, and how the books learn about it.
//!
//! - [`SettlementScheme`] is the scheme interface
//!   (spec/areas/settlement.md#the-scheme-interface), sans-IO: methods
//!   return [`Effect`]s for the node to book, rather than calling back into
//!   the node. Unlike the original's, settlements have explicit IDs, a
//!   failed one is cancelled, peer state returned by the module is kept,
//!   versions are declared for negotiation, and config is passed through.
//! - [`activation`] decides what to book when a scheme starts, from its
//!   balance and the last checkpoint, so that restarts book nothing twice.
//! - [`policy`] decides when to settle with a peer, and how much.
//! - [`Stub`] is the test-realm scheme without an underlying ledger.
//!
//! - [`ExternalAsset`] is what the node holds outside its books, such as an
//!   account on the XRP Ledger, apart from the schemes that settle through
//!   it (spec/areas/settlement.md#external-assets). It does no IO of its
//!   own: it says what to send its ledger ([`AssetEffect::Send`]) and the
//!   node feeds it what comes back ([`LedgerInput`]), then hands every
//!   scheme on it what it learnt ([`SettlementScheme::on_asset`]).
//!
//! The node keeps the registry of schemes and assets, since they live in
//! their own crates.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod activation;
mod asset;
mod commitments;
pub mod policy;
mod scheme;
mod stub;

pub use activation::{Activation, activation};
pub use asset::{
    AssetEffect, AssetId, AssetKind, Detach, ExternalAsset, Freeze, Holding, LedgerInput, Unproven,
};
pub use commitments::{Commitments, Decided, Held, HoldsWith, Membership, Promised};
pub use scheme::{
    DollarRate, Effect, EntryPayment, LedgerStatus, LossCause, MAX_OWNER_MEMO_BYTES, OwnerMemo,
    OwnerPayment, OwnerPaymentState, PaymentFailure, PegPause, PegView, PreparedSettlement,
    ReconcileCause, SchemeError, SchemeInfo, SettlementScheme,
};
pub use stub::{MAX_STUB_COMMITMENTS, STUB_INITIAL_BALANCE, Stub};
