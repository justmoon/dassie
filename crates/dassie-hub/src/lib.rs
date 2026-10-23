//! Settlement hubs (spec/designs/settlement-hubs.md), as sans-IO code.
//!
//! A hub is a small ledger kept by n = 3f + 1 validators and backed by an
//! escrow on the XRP Ledger. Two members that peer over it cannot walk away
//! with what they owe each other: the payee takes from a hold the payer set
//! aside, with the payer's signed promises, whether the payer cooperates or
//! not.
//!
//! - [`digest`]: the hashes everything in a hub is named by, each under a
//!   domain of its own.
//! - [`validators`]: a hub's validator set, its quorum and the bitmaps that
//!   say who signed.
//! - [`rules`]: the founding document whose digest is the hub ID: the
//!   sponsor's terms and the validators' acceptances.
//! - [`operation`]: what members and validators sign for the log.
//! - [`promise`]: what a payer signs for its payee on a hold, a commitment
//!   with each Prepare and a claim with each Fulfill.
//! - [`ledger`]: the hub's state and the deterministic function that takes
//!   it from one block to the next.
//! - [`client`]: what members and validators say to each other: requests,
//!   and statements a member trusts once f + 1 validators signed them.
//! - [`service`]: a validator: consensus and the ledger together, its
//!   statements, attestations and escrow signatures.
//! - [`escrow`]: the escrow on the XRP Ledger: keys, setup and check,
//!   withdrawals, what validators attest, and the payout set.
//! - [`consensus`]: Simplex, which orders the hub's log, generic over the
//!   payload of its blocks.
//! - [`member`]: a member's side: its account in the hub as an external
//!   asset, and the `hub-xrp-testnet` settlement scheme on it.
//!
//! Like the other protocol crates, it never reads a clock, an RNG or a
//! socket of its own (spec/decisions/tigerstyle.md): time and randomness
//! are passed in, and every table has a bound.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod client;
pub mod codec;
pub mod consensus;
pub mod digest;
pub mod escrow;
pub mod ledger;
pub mod member;
pub mod operation;
pub mod promise;
pub mod rules;
pub mod service;
#[cfg(feature = "sim")]
pub mod sim;
pub mod validators;

pub use digest::Digest;
pub use ledger::Ledger;
pub use rules::{Amount, HubId, Millis, Rules};
pub use validators::{MAX_VALIDATORS, Signers, ValidatorIndex, ValidatorSet};
