//! STREAM (RFC 29): money and data over Interledger, as sans-IO state
//! machines.
//!
//! - [`packet`] and [`frame`] encode and decode STREAM packets.
//! - [`crypto`] derives keys, encrypts packets and computes conditions.
//! - [`Connection`] is one end of a connection.
//! - [`Server`] issues credentials and routes incoming packets to its
//!   connections.
//!
//! Nothing here performs IO, reads a clock or draws OS randomness: callers
//! pass the time and an RNG in, so the same inputs give the same outputs
//! (spec/decisions/tigerstyle.md). The wire format is compatible with
//! `ilp-protocol-stream`; the behaviour keeps the original Dassie's
//! deliberate extensions
//! (spec/areas/stream.md#differences-from-rfc-29-and-ilp-protocol-stream) and
//! closes the RFC gaps it left (ILP-11).

#![no_std]

extern crate alloc;

pub mod allocate;
pub mod base64url;
mod buffers;
mod connection;
pub mod crypto;
mod event;
pub mod frame;
pub mod packet;
mod policy;
mod rate;
pub mod receipt;
mod server;
#[cfg(feature = "sim")]
pub mod sim;
mod stream;
pub mod token;
#[cfg(feature = "wasm")]
mod wasm;

pub use connection::{
    AssetDetails, BUFFER_LENGTH, Config, Connection, MAX_FAILURES_WITHOUT_PROGRESS, Role,
    StreamError, Transmit,
};
pub use event::{Event, Stats};
pub use frame::{ErrorCode, Frame};
pub use policy::Policy;
pub use rate::Rate;
pub use server::{Admission, ConnectionId, Credentials, Handled, Server, ServerConfig};
pub use stream::Stream;
