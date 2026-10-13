//! Canonical Octet Encoding Rules (ITU-T X.696) for the Dassie protocols.
//!
//! This is not a general ASN.1 library. It implements the subset that ILPv4,
//! ILDCP, BTP, STREAM and the Dassie peer protocol use, and it implements it
//! strictly: encoding always produces canonical OER, and decoding accepts
//! nothing else. The original's lib-oer accepted several non-canonical forms
//! (spec 4 §2, OER-q1 to OER-q3, ILP-12); see the tests named after them.
//!
//! The codec does not allocate. Decoding borrows from the input, and
//! encoding writes into a buffer the caller sizes with
//! [`Encode::encoded_len`]. Message types implement [`Encode`] and
//! [`Decode`] by hand with [`Reader`] and [`Writer`].

#![no_std]
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

#[cfg(feature = "alloc")]
extern crate alloc;

mod constraint;
mod error;
pub mod oid;
mod reader;
pub mod size;
mod writer;

pub use constraint::{Alphabet, Size};
pub use error::{DecodeError, DecodeErrorKind};
pub use oid::ObjectIdentifier;
pub use reader::Reader;
pub use writer::Writer;

/// A value with a canonical OER encoding.
pub trait Encode {
    /// Exact length of the encoding in octets.
    fn encoded_len(&self) -> usize;

    /// Writes the encoding. Implementations write exactly
    /// [`encoded_len`](Self::encoded_len) octets.
    fn encode_to(&self, writer: &mut Writer<'_>);

    /// Encodes into the start of `buffer` and returns the length written.
    ///
    /// # Panics
    ///
    /// If `buffer` is shorter than [`encoded_len`](Self::encoded_len), or if
    /// the implementation writes a different number of octets than it
    /// declared. Both are programming errors.
    fn encode(&self, buffer: &mut [u8]) -> usize {
        let length = self.encoded_len();
        assert!(buffer.len() >= length, "buffer smaller than the encoding");
        let mut writer = Writer::new(buffer);
        self.encode_to(&mut writer);
        assert!(
            writer.position() == length,
            "encoder wrote a different length than it declared"
        );
        length
    }

    /// Encodes into a new vector. For API edges such as WASM bindings and
    /// tests; protocol code encodes into buffers it already owns.
    #[cfg(feature = "alloc")]
    fn to_vec(&self) -> alloc::vec::Vec<u8> {
        let mut buffer = alloc::vec![0; self.encoded_len()];
        self.encode(&mut buffer);
        buffer
    }
}

/// A value decodable from canonical OER, borrowing from the input.
pub trait Decode<'a>: Sized {
    /// The error type. Protocol crates use their own, so that they can
    /// report failures OER knows nothing about, such as an invalid address,
    /// next to the codec's.
    type Error: From<DecodeError>;

    /// Reads one value from `reader`.
    fn decode(reader: &mut Reader<'a>) -> Result<Self, Self::Error>;

    /// Decodes a value that must occupy all of `input`.
    fn decode_exact(input: &'a [u8]) -> Result<Self, Self::Error> {
        let mut reader = Reader::new(input);
        let value = Self::decode(&mut reader)?;
        reader.finish()?;
        Ok(value)
    }
}
