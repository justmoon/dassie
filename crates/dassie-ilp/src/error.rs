use core::fmt;

use dassie_oer::{DecodeError, DecodeErrorKind};

use crate::address::InvalidAddress;

/// Why an ILP, ILDCP or BTP packet failed to parse, and where.
///
/// Parsing never panics and never throws past the caller: a
/// connector turns any of these into an F01 reject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset of the problem in the packet.
    pub offset: usize,
    /// What was wrong.
    pub kind: ParseErrorKind,
}

/// The kinds of parse failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The OER encoding is malformed or not canonical.
    Encoding(DecodeErrorKind),
    /// The type octet names no known packet type.
    UnknownType(u8),
    /// An address field is not a valid ILP address.
    InvalidAddress(InvalidAddress),
    /// An expiry is not a valid Interledger timestamp.
    InvalidTimestamp,
}

impl ParseError {
    /// Creates an error of `kind` at `offset`.
    #[must_use]
    pub const fn new(offset: usize, kind: ParseErrorKind) -> Self {
        Self { offset, kind }
    }
}

impl From<DecodeError> for ParseError {
    fn from(error: DecodeError) -> Self {
        Self::new(error.offset, ParseErrorKind::Encoding(error.kind))
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ParseErrorKind::Encoding(kind) => write!(f, "{kind}")?,
            ParseErrorKind::UnknownType(packet_type) => {
                write!(f, "unknown packet type {packet_type}")?;
            }
            ParseErrorKind::InvalidAddress(reason) => write!(f, "{reason}")?,
            ParseErrorKind::InvalidTimestamp => f.write_str("invalid Interledger timestamp")?,
        }
        write!(f, " at offset {}", self.offset)
    }
}

impl core::error::Error for ParseError {}
