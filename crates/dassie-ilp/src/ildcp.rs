//! Interledger Dynamic Configuration Protocol (RFC 31): how a child asks its
//! parent for its address and asset.

use dassie_oer::{Alphabet, Decode, Encode, Reader, Size, Writer, size};

use crate::address::{Address, MAX_ADDRESS_LENGTH};
use crate::error::{ParseError, ParseErrorKind};
use crate::packet::{Prepare, read_address};
use crate::timestamp::Timestamp;

/// Destination of ILDCP requests.
pub const ADDRESS: Address<'static> = crate::address!("peer.config");

/// The fulfillment of ILDCP requests: 32 zero octets.
pub const FULFILLMENT: [u8; 32] = [0; 32];

/// The condition of ILDCP requests, SHA-256 of [`FULFILLMENT`].
pub const CONDITION: [u8; 32] = [
    0x66, 0x68, 0x7a, 0xad, 0xf8, 0x62, 0xbd, 0x77, 0x6c, 0x8f, 0xc1, 0x8b, 0x8e, 0x9f, 0x8e, 0x20,
    0x08, 0x97, 0x14, 0x85, 0x6e, 0xe2, 0x33, 0xb3, 0x90, 0x2a, 0x59, 0x1d, 0x0d, 0x5f, 0x29, 0x25,
];

/// How long a requester waits for the response.
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;

// RFC 31 leaves the asset code unbounded. Real codes are a few characters,
// and a bound keeps a hostile parent from handing out megabytes
// (spec/decisions/tigerstyle.md).
const ASSET_CODE_SIZE: Size = Size::range(0, 255);
const ADDRESS_SIZE: Size = Size::range(1, MAX_ADDRESS_LENGTH);

/// The ILDCP request sent at `now`: a zero-amount Prepare to
/// [`ADDRESS`] with [`CONDITION`] and no data.
#[must_use]
pub const fn request(now: Timestamp) -> Prepare<'static> {
    Prepare {
        amount: 0,
        expires_at: now.saturating_add_ms(REQUEST_TIMEOUT_MS),
        execution_condition: CONDITION,
        destination: ADDRESS,
        data: &[],
    }
}

/// Whether `prepare` is an ILDCP request.
#[must_use]
pub fn is_request(prepare: &Prepare<'_>) -> bool {
    prepare.destination == ADDRESS && prepare.execution_condition == CONDITION
}

/// The configuration a parent hands its child, carried in the Fulfill data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Response<'a> {
    /// The child's ILP address.
    pub address: Address<'a>,
    /// Decimal places of the asset: an amount of 1 is 10^-scale units.
    pub asset_scale: u8,
    /// The asset's code, for example `XRP` or `USD`.
    pub asset_code: &'a str,
}

impl Encode for Response<'_> {
    fn encoded_len(&self) -> usize {
        size::prefixed(self.address.as_str().len()) + 1 + size::prefixed(self.asset_code.len())
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_ascii(self.address.as_str(), ADDRESS_SIZE, Alphabet::IA5);
        writer.write_u8(self.asset_scale);
        writer.write_ascii(self.asset_code, ASSET_CODE_SIZE, Alphabet::IA5);
    }
}

impl<'a> Decode<'a> for Response<'a> {
    type Error = ParseError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, ParseError> {
        let offset = reader.offset();
        let address = read_address(reader, ADDRESS_SIZE)?.ok_or(ParseError::new(
            offset,
            ParseErrorKind::Encoding(dassie_oer::DecodeErrorKind::SizeOutOfRange),
        ))?;
        Ok(Self {
            address,
            asset_scale: reader.read_u8()?,
            asset_code: reader.read_ascii(ASSET_CODE_SIZE, Alphabet::IA5)?,
        })
    }
}
