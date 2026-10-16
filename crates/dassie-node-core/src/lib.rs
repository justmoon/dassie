//! The Dassie node's decisions, without IO.
//!
//! Everything the node decides about its owner's access and its own
//! certificate lives here:
//!
//! - [`config`]: validated configuration values;
//! - [`urls`]: the URLs the node prints and redirects to;
//! - [`setup`]: the one-time setup token and the setup and login checks;
//! - [`session`]: session tokens, lifetimes and cookies;
//! - [`origin`]: the same-origin rule for the owner's endpoints;
//! - [`rate_limit`]: a bounded token-bucket table for login attempts;
//! - [`connections`]: a bounded table of open connections, per client;
//! - [`certificate`]: which certificate to serve, and when to replace it;
//! - [`acme`]: when to renew, how to back off, and the challenge tokens.
//!
//! `dassie-node` carries the decisions out against sockets, files and the
//! database. Nothing here reads a clock, draws OS randomness or touches the
//! network: time comes in as `now_ms` (milliseconds since the Unix epoch)
//! and randomness from an RNG the caller passes in, so the same inputs give
//! the same outputs (D16).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod acme;
pub mod address;
pub mod certificate;
pub mod config;
pub mod connections;
pub mod hex;
pub mod origin;
pub mod rate_limit;
pub mod session;
pub mod setup;
#[cfg(feature = "testing")]
pub mod testing;
pub mod urls;

/// One second in milliseconds.
pub const SECOND_MS: u64 = 1_000;
/// One minute in milliseconds.
pub const MINUTE_MS: u64 = 60 * SECOND_MS;
/// One hour in milliseconds.
pub const HOUR_MS: u64 = 60 * MINUTE_MS;
/// One day in milliseconds.
pub const DAY_MS: u64 = 24 * HOUR_MS;
