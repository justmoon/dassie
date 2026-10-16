//! Interledger Protocol v4 (RFC 27) packets, addresses (RFC 15) and
//! timestamps, plus the two protocols that ride on them in Dassie: ILDCP
//! (RFC 31) and BTP 2.0 (RFC 23).
//!
//! Everything here is byte-for-byte compatible with the standards and with
//! the original Dassie (spec 7 §2). Parsing returns a [`ParseError`] for any
//! malformed input instead of throwing (ILP-3), decodes without allocating by
//! borrowing from the input, and validates addresses and timestamps
//! strictly.

#![no_std]

mod address;
mod amount_too_large;
pub mod btp;
mod error;
mod error_code;
pub mod ildcp;
mod packet;
mod timestamp;
#[cfg(feature = "wasm")]
mod wasm;

pub use address::{Address, InvalidAddress, MAX_ADDRESS_LENGTH};
pub use amount_too_large::AmountTooLarge;
pub use error::{ParseError, ParseErrorKind};
pub use error_code::{ErrorClass, ErrorCode};
pub use packet::{
    Fulfill, MAX_DATA_LENGTH, MAX_MESSAGE_LENGTH, MAX_PACKET_LENGTH, Packet, Prepare, Reject,
    Response, TYPE_FULFILL, TYPE_PREPARE, TYPE_REJECT, condition_for,
};
pub use timestamp::{InvalidTimestamp, TIMESTAMP_LENGTH, Timestamp};
