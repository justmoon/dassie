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
//!
//! Like the other protocol crates, it never reads a clock, an RNG or a
//! socket of its own (spec/decisions/tigerstyle.md): time and randomness
//! are passed in, and every table has a bound.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod digest;
pub mod validators;

pub use digest::Digest;
pub use validators::{MAX_VALIDATORS, Signers, ValidatorIndex, ValidatorSet};
