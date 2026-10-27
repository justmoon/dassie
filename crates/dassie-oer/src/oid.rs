//! OBJECT IDENTIFIER: a length determinant followed by the BER content
//! octets (X.696 §8.19, X.690 §8.19).

use crate::error::{DecodeError, DecodeErrorKind};
use crate::reader::Reader;
use crate::size;
use crate::writer::Writer;

/// Most arcs an object identifier may have. X.660 sets no limit, but nothing
/// Dassie uses comes close, and the bound keeps iteration finite.
pub const MAX_ARCS: usize = 32;

/// A decoded object identifier, borrowing its validated content octets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectIdentifier<'a> {
    content: &'a [u8],
}

impl<'a> ObjectIdentifier<'a> {
    /// Reads a length-prefixed object identifier and validates its content.
    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let length = reader.read_length()?;
        let invalid = reader.error(DecodeErrorKind::InvalidObjectIdentifier);
        let content = reader.read_bytes(length)?;
        let mut inner = Reader::new(content);
        let mut subidentifiers = 0;
        while !inner.is_empty() {
            // The first subidentifier holds two arcs.
            subidentifiers += 1;
            if subidentifiers + 1 > MAX_ARCS {
                return Err(invalid);
            }
            inner.read_base128().map_err(|_| invalid)?;
        }
        if subidentifiers == 0 {
            return Err(invalid);
        }
        Ok(Self { content })
    }

    /// The BER content octets, without the length determinant.
    #[must_use]
    pub const fn content(&self) -> &'a [u8] {
        self.content
    }

    /// Writes the arcs into `arcs` and returns how many there are. The first
    /// subidentifier holds two arcs (X.690 §8.19.4).
    #[must_use]
    pub fn arcs(&self, arcs: &mut [u64; MAX_ARCS]) -> usize {
        let mut reader = Reader::new(self.content);
        let mut count = 0;
        while !reader.is_empty() && count < MAX_ARCS {
            // Validated by `decode`, so reading cannot fail.
            let Ok(subidentifier) = reader.read_base128() else {
                break;
            };
            if count == 0 {
                let first = if subidentifier < 80 {
                    subidentifier / 40
                } else {
                    2
                };
                if let Some(slot) = arcs.get_mut(0) {
                    *slot = first;
                }
                if let Some(slot) = arcs.get_mut(1) {
                    *slot = subidentifier - first * 40;
                }
                count = 2;
            } else {
                if let Some(slot) = arcs.get_mut(count) {
                    *slot = subidentifier;
                }
                count += 1;
            }
        }
        count
    }
}

/// The value of the first subidentifier, which combines the first two arcs.
// Arcs that are no object identifier are the caller's bug: they are the
// protocol's constants, mostly evaluated when the crate builds.
#[allow(clippy::panic)]
const fn first_subidentifier(arcs: &[u64]) -> u64 {
    let [first, second, ..] = arcs else {
        panic!("an object identifier needs at least two arcs");
    };
    assert!(*first <= 2, "the first arc must be 0, 1 or 2");
    assert!(
        *first == 2 || *second < 40,
        "the second arc must be below 40"
    );
    match (*first * 40).checked_add(*second) {
        Some(value) => value,
        None => panic!("the first subidentifier overflows"),
    }
}

/// Length of the content octets for `arcs`.
#[must_use]
pub const fn content_len(arcs: &[u64]) -> usize {
    assert!(arcs.len() <= MAX_ARCS, "too many arcs");
    let mut length = size::base128(first_subidentifier(arcs));
    let mut rest = arcs;
    // Skip the first two arcs, which the first subidentifier covers.
    if let [_, _, tail @ ..] = rest {
        rest = tail;
    }
    while let [arc, tail @ ..] = rest {
        length += size::base128(*arc);
        rest = tail;
    }
    length
}

/// Encoded length (length determinant and content) of `arcs`.
#[must_use]
pub const fn encoded_len(arcs: &[u64]) -> usize {
    size::prefixed(content_len(arcs))
}

/// Writes `arcs` as a length-prefixed object identifier.
pub fn write(writer: &mut Writer<'_>, arcs: &[u64]) {
    writer.write_prefixed(content_len(arcs), |writer| {
        writer.write_base128(first_subidentifier(arcs));
        for &arc in arcs.get(2..).unwrap_or_default() {
            writer.write_base128(arc);
        }
    });
}

/// The complete encoding (length determinant and content) of an object
/// identifier, computed at compile time for constants (the golden vectors'
/// test, `tests/vectors.rs`, holds Dassie's own to it):
///
/// ```ignore
/// const DASSIE: [u8; 12] = dassie_oer::oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399]);
/// assert_eq!(DASSIE[0], 11);
/// ```
#[must_use]
pub const fn encode<const N: usize>(arcs: &[u64]) -> [u8; N] {
    let content = content_len(arcs);
    assert!(content < 0x80, "constant object identifiers must be short");
    assert!(N == content + 1, "N must be the encoded length");
    let mut output = [0u8; N];
    #[allow(clippy::cast_possible_truncation)]
    {
        // Checked above: the content length fits in seven bits.
        output[0] = content as u8;
    }
    let mut position = put_base128(&mut output, 1, first_subidentifier(arcs));
    let mut rest = arcs;
    if let [_, _, tail @ ..] = rest {
        rest = tail;
    }
    while let [arc, tail @ ..] = rest {
        position = put_base128(&mut output, position, *arc);
        rest = tail;
    }
    assert!(position == N, "object identifier length mismatch");
    output
}

#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
const fn put_base128<const N: usize>(output: &mut [u8; N], start: usize, value: u64) -> usize {
    // Const evaluation fails on out-of-bounds indexing, so this cannot write
    // past the array; the caller asserts the total length.
    let groups = size::base128(value);
    let mut index = 0;
    while index < groups {
        let shift = 7 * (groups - 1 - index);
        let continuation = if index + 1 == groups { 0 } else { 0x80 };
        output[start + index] = ((value >> shift) & 0x7f) as u8 | continuation;
        index += 1;
    }
    start + groups
}
