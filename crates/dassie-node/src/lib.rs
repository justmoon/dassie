//! The Dassie daemon.

// Allowed in the daemon, denied in the engine's core (see clippy.toml).
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

pub mod acme;
pub mod clock;
pub mod env;
pub mod log;
pub mod node;
pub mod open_payments;
pub mod peer_client;
pub mod rates;
pub mod rpc;
pub mod store;
pub mod tls;

/// The version of this build: the release or canary it was built as
/// (`DASSIE_VERSION`, set by the release build), or the crate's.
pub const VERSION: &str = match option_env!("DASSIE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
pub mod daemon;
pub mod engine;
#[cfg(feature = "testing")]
pub mod fake_ledger;
pub mod http;
pub mod ipc;
pub mod supervisor;
pub mod systemd;
