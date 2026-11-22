//! `methodOffer`: the settlement methods a node settles on with one peer
//! (spec/decisions/method-offers.md,
//! spec/areas/peer-protocol.md#message-set).
//!
//! ```text
//! MethodOffer ::= SEQUENCE {
//!   version  UInt64,
//!   seen     UInt64,
//!   methods  SEQUENCE (SIZE(0..MAX_OFFERS)) OF SEQUENCE {
//!     scheme SchemeId,
//!     data   OCTET STRING (SIZE(0..MAX_OFFER_DETAILS))
//!   }
//! }
//! ```
//!
//! The request carries the sender's offer; a 200 answer is the recipient's
//! own, in the same layout.

use alloc::vec::Vec;

use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, size};

use super::{MAX_OFFER_DETAILS, MAX_OFFERS, SchemeId};

/// The size of a method's data: the scheme's own, as a link state's offer
/// details are bounded.
const DATA_SIZE: Size = Size::range(0, MAX_OFFER_DETAILS);

/// One method offered: a scheme, with the scheme's own data for the peer,
/// in its canonical OER (over a settlement hub, the node's membership).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfferedMethod<'a> {
    /// The scheme.
    pub scheme: SchemeId,
    /// The scheme's data, at most [`MAX_OFFER_DETAILS`] bytes.
    pub data: &'a [u8],
}

/// A node's offer to one peer: the methods it settles on with it, at a
/// version of its own for the pair, with the newest of the peer's versions
/// it has taken in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodOffer<'a> {
    /// The sender's version for this pair: it rises by one with each
    /// change, and never starts again.
    pub version: u64,
    /// The newest of the recipient's versions the sender has taken in, 0
    /// for none.
    pub seen: u64,
    /// The methods, no scheme twice, at most [`MAX_OFFERS`].
    pub methods: Vec<OfferedMethod<'a>>,
}

impl Encode for MethodOffer<'_> {
    fn encoded_len(&self) -> usize {
        8 + 8
            + size::quantity(self.methods.len())
            + self
                .methods
                .iter()
                .map(|method| method.scheme.encoded_len() + size::prefixed(method.data.len()))
                .sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        assert!(self.methods.len() <= MAX_OFFERS, "an offer the wire takes");
        writer.write_u64(self.version);
        writer.write_u64(self.seen);
        writer.write_quantity(self.methods.len());
        for method in &self.methods {
            method.scheme.write(writer);
            writer.write_octets(method.data, DATA_SIZE);
        }
    }
}

impl<'a> Decode<'a> for MethodOffer<'a> {
    type Error = DecodeError;

    /// Reads an offer; one that names a scheme twice is malformed.
    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let version = reader.read_u64()?;
        let seen = reader.read_u64()?;
        let count = reader.read_quantity(MAX_OFFERS)?;
        let mut methods: Vec<OfferedMethod<'a>> = Vec::with_capacity(count);
        for _ in 0..count {
            let offset = reader.offset();
            let scheme = SchemeId::read(reader)?;
            if methods.iter().any(|method| method.scheme == scheme) {
                return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedValue));
            }
            methods.push(OfferedMethod {
                scheme,
                data: reader.read_octets(DATA_SIZE)?,
            });
        }
        Ok(Self {
            version,
            seen,
            methods,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn scheme(id: &str) -> SchemeId {
        SchemeId::parse(id).unwrap()
    }

    #[test]
    fn an_offer_reads_back() {
        let offer = MethodOffer {
            version: 7,
            seen: 3,
            methods: alloc::vec![
                OfferedMethod {
                    scheme: scheme("stub"),
                    data: &[1],
                },
                OfferedMethod {
                    scheme: scheme("stub-two"),
                    data: &[],
                },
            ],
        };
        let bytes = offer.to_vec();
        assert_eq!(bytes.len(), offer.encoded_len());
        assert_eq!(MethodOffer::decode_exact(&bytes).unwrap(), offer);
    }

    // spec/areas/peer-protocol.md#message-set: an offer names each scheme
    // once; one that names a scheme twice is malformed.
    #[test]
    fn a_scheme_offered_twice_is_malformed() {
        let offer = MethodOffer {
            version: 1,
            seen: 0,
            methods: alloc::vec![
                OfferedMethod {
                    scheme: scheme("stub"),
                    data: &[],
                },
                OfferedMethod {
                    scheme: scheme("stub"),
                    data: &[2],
                },
            ],
        };
        assert!(MethodOffer::decode_exact(&offer.to_vec()).is_err());
    }

    #[test]
    fn data_past_its_bound_is_malformed() {
        let mut bytes = MethodOffer {
            version: 1,
            seen: 0,
            methods: alloc::vec![OfferedMethod {
                scheme: scheme("stub"),
                data: &[0; MAX_OFFER_DETAILS],
            }],
        }
        .to_vec();
        assert!(MethodOffer::decode_exact(&bytes).is_ok());
        // The same with one byte more: its length, 256 in three bytes,
        // becomes 257.
        let at = bytes.len() - MAX_OFFER_DETAILS - 1;
        assert_eq!(bytes[at], 0, "the length's last byte");
        bytes[at] = 1;
        bytes.push(0);
        assert!(MethodOffer::decode_exact(&bytes).is_err());
    }
}
