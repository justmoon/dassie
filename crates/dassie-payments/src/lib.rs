//! Open Payments (v1.4.0) without IO.
//!
//! - [`resources`] and [`amount`]: the JSON of wallet addresses, incoming
//!   and outgoing payments, quotes, pages and errors; [`quote`]: what a
//!   payment debits and delivers at a rate.
//! - [`gnap`]: grant requests, the rules on them, access checks and the
//!   interaction hash.
//! - [`httpsig`]: HTTP message signatures (RFC 9421) and `Content-Digest`
//!   (RFC 9530), over [`sf`], structured field values (RFC 8941).
//! - [`jwk`]: Ed25519 keys as JSON Web Keys.
//! - [`pointer`] and [`uri`]: payment pointers, wallet address URLs, and
//!   the parts of a URL that signatures cover.
//! - [`time`]: RFC 3339 timestamps and calendar arithmetic; [`interval`]:
//!   ISO 8601 durations and repeating intervals, for grant limits.
//!
//! `dassie-node` serves and calls Open Payments with these. Nothing here
//! reads a clock or touches the network: time comes in as an argument, so
//! the same inputs give the same outputs (spec/decisions/tigerstyle.md).

#![no_std]

extern crate alloc;

pub mod amount;
pub mod gnap;
pub mod httpsig;
pub mod interval;
pub mod jwk;
pub mod pointer;
pub mod quote;
pub mod resources;
pub mod sf;
pub mod time;
pub mod uri;

pub use ed25519_dalek::{SigningKey, VerifyingKey};
