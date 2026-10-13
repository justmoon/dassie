//! The Dassie daemon.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

pub mod acme;
pub mod clock;
pub mod env;
pub mod log;
pub mod node;
pub mod rpc;
pub mod store;
pub mod tls;

/// The version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod daemon;
pub mod engine;
pub mod http;
pub mod ipc;
pub mod supervisor;
pub mod systemd;
