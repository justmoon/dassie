//! The Dassie daemon.

// Allowed in the daemon, denied in the engine's core (see clippy.toml).
#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]

pub mod acme;
pub mod clock;
pub mod env;
pub mod log;
pub mod node;
pub mod open_payments;
pub mod outbound;
pub mod owner_app;
pub mod peer_server;
pub mod rates;
pub mod rpc;
pub mod slots;
pub mod store;
pub mod tls;

/// The version of this build: the release it was built as
/// (`DASSIE_BUILD_VERSION`, set by the release build), or the crate's. Not
/// `DASSIE_VERSION`, which an operator may have exported for something else.
pub const VERSION: &str = match option_env!("DASSIE_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
pub mod daemon;
pub mod engine;
pub mod event_log;
#[cfg(feature = "testing")]
pub mod fake_ledger;
pub mod feed;
pub mod front_door;
pub mod http;
pub mod hub;
pub mod ipc;
pub mod quic;
#[cfg(feature = "testing")]
pub mod sim;
pub mod supervisor;
pub mod systemd;
