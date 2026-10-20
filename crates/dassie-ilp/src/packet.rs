use dassie_oer::{Alphabet, Decode, Encode, Reader, Size, Writer, size};
use sha2::{Digest, Sha256};

use crate::address::{Address, MAX_ADDRESS_LENGTH};
use crate::error::{ParseError, ParseErrorKind};
use crate::error_code::ErrorCode;
use crate::timestamp::{TIMESTAMP_LENGTH, Timestamp};

/// Type octet of an ILP Prepare.
pub const TYPE_PREPARE: u8 = 12;
/// Type octet of an ILP Fulfill.
pub const TYPE_FULFILL: u8 = 13;
/// Type octet of an ILP Reject.
pub const TYPE_REJECT: u8 = 14;

/// Largest `data` field of any ILP packet, in octets.
pub const MAX_DATA_LENGTH: usize = 32_767;
/// Largest reject `message`, in octets. RFC 27 specifies 8191 octets; the
/// original counted UTF-16 code units instead
/// (spec/original/04-interledger.md#14-reject-type-14).
pub const MAX_MESSAGE_LENGTH: usize = 8_191;
/// Largest encoding of any ILP packet, which callers can use to size
/// buffers once. It is a Reject's, with the longest `triggeredBy`, message
/// and data: a connector passes on whatever Reject decodes, and it is
/// larger than the largest Prepare.
pub const MAX_PACKET_LENGTH: usize = max(
    max(MAX_PREPARE_LENGTH, MAX_FULFILL_LENGTH),
    MAX_REJECT_LENGTH,
);

const MAX_PREPARE_LENGTH: usize = packet_length(
    8 + TIMESTAMP_LENGTH
        + 32
        + size::prefixed(MAX_ADDRESS_LENGTH)
        + size::prefixed(MAX_DATA_LENGTH),
);

const MAX_FULFILL_LENGTH: usize = packet_length(32 + size::prefixed(MAX_DATA_LENGTH));

const MAX_REJECT_LENGTH: usize = packet_length(
    3 + size::prefixed(MAX_ADDRESS_LENGTH)
        + size::prefixed(MAX_MESSAGE_LENGTH)
        + size::prefixed(MAX_DATA_LENGTH),
);

/// The encoding of a packet with a body of `body_length` octets: its type
/// octet, the body's length, and the body.
const fn packet_length(body_length: usize) -> usize {
    1 + size::length_determinant(body_length) + body_length
}

const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

const ADDRESS_SIZE: Size = Size::range(1, MAX_ADDRESS_LENGTH);
const OPTIONAL_ADDRESS_SIZE: Size = Size::range(0, MAX_ADDRESS_LENGTH);
const DATA_SIZE: Size = Size::range(0, MAX_DATA_LENGTH);
const MESSAGE_SIZE: Size = Size::range(0, MAX_MESSAGE_LENGTH);

/// The condition (SHA-256 preimage hash) a fulfillment satisfies.
#[must_use]
pub fn condition_for(fulfillment: &[u8; 32]) -> [u8; 32] {
    Sha256::digest(fulfillment).into()
}

/// An ILP Prepare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prepare<'a> {
    /// Amount in the receiving account's units.
    pub amount: u64,
    /// When the sender stops waiting for a response.
    pub expires_at: Timestamp,
    /// SHA-256 hash of the fulfillment that releases the payment.
    pub execution_condition: [u8; 32],
    /// Where the packet is going.
    pub destination: Address<'a>,
    /// Application data, at most [`MAX_DATA_LENGTH`] octets.
    pub data: &'a [u8],
}

/// An ILP Fulfill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fulfill<'a> {
    /// The preimage of the Prepare's execution condition.
    pub fulfillment: [u8; 32],
    /// Application data, at most [`MAX_DATA_LENGTH`] octets.
    pub data: &'a [u8],
}

/// An ILP Reject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reject<'a> {
    /// Why the packet was rejected.
    pub code: ErrorCode,
    /// Who rejected it, if they said. ILPv4 allows an empty address here;
    /// the original required one (ILP-3, spec/areas/ilp.md#packets).
    pub triggered_by: Option<Address<'a>>,
    /// A human-readable explanation, at most [`MAX_MESSAGE_LENGTH`] octets.
    pub message: &'a str,
    /// Machine-readable details, at most [`MAX_DATA_LENGTH`] octets.
    pub data: &'a [u8],
}

/// Any ILP packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Packet<'a> {
    /// A Prepare.
    Prepare(Prepare<'a>),
    /// A Fulfill.
    Fulfill(Fulfill<'a>),
    /// A Reject.
    Reject(Reject<'a>),
}

/// The answer to a Prepare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response<'a> {
    /// The receiver fulfilled the Prepare.
    Fulfill(Fulfill<'a>),
    /// Someone on the path rejected it.
    Reject(Reject<'a>),
}

/// Shared by the three packet types: the encoding up to and including the
/// length determinant of `data`, for `data_len` octets of data.
///
/// The `encode_header` methods exist so that a caller can place the data
/// after the header itself, for example to encrypt it in place (STREAM
/// does), without the packet holding it.
fn header_len(fixed_body_len: usize, data_len: usize) -> usize {
    let body_len = fixed_body_len + size::prefixed(data_len);
    1 + size::length_determinant(body_len) + body_len - data_len
}

impl Prepare<'_> {
    fn fixed_body_len(&self) -> usize {
        8 + TIMESTAMP_LENGTH + 32 + size::prefixed(self.destination.as_str().len())
    }

    /// Length of the encoding without the `data_len` octets of data, which
    /// follow it. `self.data` is ignored.
    #[must_use]
    pub fn header_len(&self, data_len: usize) -> usize {
        header_len(self.fixed_body_len(), data_len)
    }

    /// Writes the encoding up to the data, for `data_len` octets of data
    /// that the caller places right after it. `self.data` is ignored.
    pub fn encode_header(&self, data_len: usize, writer: &mut Writer<'_>) {
        assert!(data_len <= MAX_DATA_LENGTH, "ILP data too long");
        writer.write_u8(TYPE_PREPARE);
        writer.write_length(self.fixed_body_len() + size::prefixed(data_len));
        writer.write_u64(self.amount);
        writer.write_bytes(&self.expires_at.to_ilp_bytes());
        writer.write_bytes(&self.execution_condition);
        writer.write_ascii(self.destination.as_str(), ADDRESS_SIZE, Alphabet::IA5);
        writer.write_length(data_len);
    }
}

impl Fulfill<'_> {
    /// Whether this fulfillment satisfies `condition`.
    #[must_use]
    pub fn satisfies(&self, condition: &[u8; 32]) -> bool {
        condition_for(&self.fulfillment) == *condition
    }

    /// Length of the encoding without the `data_len` octets of data, which
    /// follow it. `self.data` is ignored.
    #[must_use]
    pub fn header_len(&self, data_len: usize) -> usize {
        header_len(32, data_len)
    }

    /// Writes the encoding up to the data, for `data_len` octets of data
    /// that the caller places right after it. `self.data` is ignored.
    pub fn encode_header(&self, data_len: usize, writer: &mut Writer<'_>) {
        assert!(data_len <= MAX_DATA_LENGTH, "ILP data too long");
        writer.write_u8(TYPE_FULFILL);
        writer.write_length(32 + size::prefixed(data_len));
        writer.write_bytes(&self.fulfillment);
        writer.write_length(data_len);
    }
}

impl Reject<'_> {
    fn fixed_body_len(&self) -> usize {
        let triggered_by = self
            .triggered_by
            .map_or(0, |address| address.as_str().len());
        3 + size::prefixed(triggered_by) + size::prefixed(self.message.len())
    }

    /// Length of the encoding without the `data_len` octets of data, which
    /// follow it. `self.data` is ignored.
    #[must_use]
    pub fn header_len(&self, data_len: usize) -> usize {
        header_len(self.fixed_body_len(), data_len)
    }

    /// Writes the encoding up to the data, for `data_len` octets of data
    /// that the caller places right after it. `self.data` is ignored.
    pub fn encode_header(&self, data_len: usize, writer: &mut Writer<'_>) {
        assert!(data_len <= MAX_DATA_LENGTH, "ILP data too long");
        writer.write_u8(TYPE_REJECT);
        writer.write_length(self.fixed_body_len() + size::prefixed(data_len));
        writer.write_bytes(&self.code.bytes());
        let triggered_by = self.triggered_by.map_or("", |address| address.as_str());
        writer.write_ascii(triggered_by, OPTIONAL_ADDRESS_SIZE, Alphabet::IA5);
        writer.write_utf8(self.message, MESSAGE_SIZE);
        writer.write_length(data_len);
    }
}

impl Encode for Prepare<'_> {
    fn encoded_len(&self) -> usize {
        self.header_len(self.data.len()) + self.data.len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.encode_header(self.data.len(), writer);
        writer.write_bytes(self.data);
    }
}

impl Encode for Fulfill<'_> {
    fn encoded_len(&self) -> usize {
        self.header_len(self.data.len()) + self.data.len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.encode_header(self.data.len(), writer);
        writer.write_bytes(self.data);
    }
}

impl Encode for Reject<'_> {
    fn encoded_len(&self) -> usize {
        self.header_len(self.data.len()) + self.data.len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.encode_header(self.data.len(), writer);
        writer.write_bytes(self.data);
    }
}

impl Encode for Packet<'_> {
    fn encoded_len(&self) -> usize {
        match self {
            Self::Prepare(prepare) => prepare.encoded_len(),
            Self::Fulfill(fulfill) => fulfill.encoded_len(),
            Self::Reject(reject) => reject.encoded_len(),
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Prepare(prepare) => prepare.encode_to(writer),
            Self::Fulfill(fulfill) => fulfill.encode_to(writer),
            Self::Reject(reject) => reject.encode_to(writer),
        }
    }
}

impl Encode for Response<'_> {
    fn encoded_len(&self) -> usize {
        match self {
            Self::Fulfill(fulfill) => fulfill.encoded_len(),
            Self::Reject(reject) => reject.encoded_len(),
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Fulfill(fulfill) => fulfill.encode_to(writer),
            Self::Reject(reject) => reject.encode_to(writer),
        }
    }
}

/// Reads an IA5String of `size` and validates it as an ILP address.
pub(crate) fn read_address<'a>(
    reader: &mut Reader<'a>,
    size: Size,
) -> Result<Option<Address<'a>>, ParseError> {
    let offset = reader.offset();
    let text = reader.read_ascii(size, Alphabet::IA5)?;
    if text.is_empty() {
        return Ok(None);
    }
    Address::parse(text)
        .map(Some)
        .map_err(|reason| ParseError::new(offset, ParseErrorKind::InvalidAddress(reason)))
}

fn read_prepare_body<'a>(reader: &mut Reader<'a>) -> Result<Prepare<'a>, ParseError> {
    let amount = reader.read_u64()?;
    let expiry_offset = reader.offset();
    let expires_at = Timestamp::parse(reader.read_array()?)
        .map_err(|_| ParseError::new(expiry_offset, ParseErrorKind::InvalidTimestamp))?;
    let execution_condition = *reader.read_array()?;
    let destination_offset = reader.offset();
    let destination = read_address(reader, ADDRESS_SIZE)?.ok_or(ParseError::new(
        destination_offset,
        ParseErrorKind::Encoding(dassie_oer::DecodeErrorKind::SizeOutOfRange),
    ))?;
    let data = reader.read_octets(DATA_SIZE)?;
    Ok(Prepare {
        amount,
        expires_at,
        execution_condition,
        destination,
        data,
    })
}

fn read_fulfill_body<'a>(reader: &mut Reader<'a>) -> Result<Fulfill<'a>, ParseError> {
    let fulfillment = *reader.read_array()?;
    let data = reader.read_octets(DATA_SIZE)?;
    Ok(Fulfill { fulfillment, data })
}

fn read_reject_body<'a>(reader: &mut Reader<'a>) -> Result<Reject<'a>, ParseError> {
    let code_offset = reader.offset();
    let code = ErrorCode::new(*reader.read_array()?).ok_or(ParseError::new(
        code_offset,
        ParseErrorKind::Encoding(dassie_oer::DecodeErrorKind::InvalidCharacter),
    ))?;
    let triggered_by = read_address(reader, OPTIONAL_ADDRESS_SIZE)?;
    let message = reader.read_utf8(MESSAGE_SIZE)?;
    let data = reader.read_octets(DATA_SIZE)?;
    Ok(Reject {
        code,
        triggered_by,
        message,
        data,
    })
}

/// Reads the type octet and checks it is one of `allowed`.
fn read_type(reader: &mut Reader<'_>, allowed: &[u8]) -> Result<u8, ParseError> {
    let offset = reader.offset();
    let packet_type = reader.read_u8()?;
    if allowed.contains(&packet_type) {
        Ok(packet_type)
    } else {
        Err(ParseError::new(
            offset,
            ParseErrorKind::UnknownType(packet_type),
        ))
    }
}

impl<'a> Decode<'a> for Packet<'a> {
    type Error = ParseError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, ParseError> {
        match read_type(reader, &[TYPE_PREPARE, TYPE_FULFILL, TYPE_REJECT])? {
            TYPE_PREPARE => Ok(Self::Prepare(reader.read_prefixed(read_prepare_body)?)),
            TYPE_FULFILL => Ok(Self::Fulfill(reader.read_prefixed(read_fulfill_body)?)),
            _ => Ok(Self::Reject(reader.read_prefixed(read_reject_body)?)),
        }
    }
}

impl<'a> Decode<'a> for Prepare<'a> {
    type Error = ParseError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, ParseError> {
        read_type(reader, &[TYPE_PREPARE])?;
        reader.read_prefixed(read_prepare_body)
    }
}

impl<'a> Decode<'a> for Response<'a> {
    type Error = ParseError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, ParseError> {
        match read_type(reader, &[TYPE_FULFILL, TYPE_REJECT])? {
            TYPE_FULFILL => Ok(Self::Fulfill(reader.read_prefixed(read_fulfill_body)?)),
            _ => Ok(Self::Reject(reader.read_prefixed(read_reject_body)?)),
        }
    }
}
