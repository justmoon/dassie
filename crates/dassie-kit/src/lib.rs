//! What Dassie's crates share below every protocol, and which belongs to
//! none of them:
//!
//! - the units of time, in milliseconds;
//! - [`ClientKey`]: who a request comes from, for per-client bounds;
//! - [`sim`] (feature `sim`): the simulation kit, a seeded RNG, how a run
//!   ended and running a range of seeds
//!   (spec/decisions/random-seed-simulations.md), for tests and the
//!   crates' simulations.
//!
//! It depends on no other crate of the workspace, so a protocol crate can
//! use it without taking on the node's policies.

#![no_std]

#[cfg(feature = "sim")]
extern crate alloc;
// The simulation kit catches a run's panic, which needs std.
#[cfg(feature = "sim")]
extern crate std;

mod client;
#[cfg(feature = "sim")]
pub mod sim;

pub use client::ClientKey;

/// One second in milliseconds.
pub const SECOND_MS: u64 = 1_000;
/// One minute in milliseconds.
pub const MINUTE_MS: u64 = 60 * SECOND_MS;
/// One hour in milliseconds.
pub const HOUR_MS: u64 = 60 * MINUTE_MS;
/// One day in milliseconds.
pub const DAY_MS: u64 = 24 * HOUR_MS;
