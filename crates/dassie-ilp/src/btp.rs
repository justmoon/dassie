//! Bilateral Transfer Protocol 2.0 (RFC 23) packets, which carry ILP over a
//! WebSocket between a node and its BTP clients.

use dassie_oer::{Alphabet, Decode, DecodeError, Encode, Reader, Size, Writer, size};

use crate::error::{ParseError, ParseErrorKind};

/// Most protocol data entries a packet may carry. RFC 23 sets no limit;
/// real clients send one or two (spec/decisions/tigerstyle.md).
pub const MAX_PROTOCOL_DATA_ENTRIES: usize = 16;
/// Largest `data` of a BTP Error, in octets (RFC 23).
pub const MAX_ERROR_DATA_LENGTH: usize = 8192;

const PROTOCOL_NAME_SIZE: Size = Size::range(0, 255);
const TEXT_SIZE: Size = Size::range(0, 8192);
const ERROR_DATA_SIZE: Size = Size::range(0, MAX_ERROR_DATA_LENGTH);

/// BTP packet types (RFC 23 §"Message types").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    /// Answers a Message or Transfer.
    Response = 1,
    /// Answers a Message or Transfer that failed.
    Error = 2,
    /// Carries protocol data, such as an ILP packet.
    Message = 6,
    /// Carries an amount and protocol data.
    Transfer = 7,
}

impl PacketType {
    const fn from_octet(octet: u8) -> Option<Self> {
        match octet {
            1 => Some(Self::Response),
            2 => Some(Self::Error),
            6 => Some(Self::Message),
            7 => Some(Self::Transfer),
            _ => None,
        }
    }
}

/// Content types of protocol data entries. Unknown values pass through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentType(pub u8);

impl ContentType {
    /// `application/octet-stream`.
    pub const OCTET_STREAM: Self = Self(0);
    /// `text/plain; charset=utf-8`.
    pub const TEXT_PLAIN_UTF8: Self = Self(1);
    /// `application/json`.
    pub const JSON: Self = Self(2);
}

/// One protocol data entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolDataEntry<'a> {
    /// Names the subprotocol, for example `ilp` or `auth_token`.
    pub protocol_name: &'a str,
    /// How to interpret `data`.
    pub content_type: ContentType,
    /// The payload.
    pub data: &'a [u8],
}

/// The protocol data of a packet: either entries the caller built, or the
/// validated encoding of decoded entries, iterated without allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolData<'a> {
    /// Entries to encode.
    Entries(&'a [ProtocolDataEntry<'a>]),
    /// Decoded entries: their count and their concatenated encodings.
    Encoded {
        /// Number of entries.
        count: usize,
        /// The entries' encodings, already validated.
        bytes: &'a [u8],
    },
}

impl<'a> ProtocolData<'a> {
    /// No entries.
    pub const EMPTY: Self = Self::Entries(&[]);

    /// Number of entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::Entries(entries) => entries.len(),
            Self::Encoded { count, .. } => *count,
        }
    }

    /// Whether there are no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The entries in order.
    pub fn iter(&self) -> impl Iterator<Item = ProtocolDataEntry<'a>> + 'a {
        let (entries, encoded) = match *self {
            Self::Entries(entries) => (entries, None),
            Self::Encoded { count, bytes } => (&[][..], Some((count, Reader::new(bytes)))),
        };
        let mut encoded = encoded;
        entries.iter().copied().chain(core::iter::from_fn(move || {
            let (remaining, reader) = encoded.as_mut()?;
            if *remaining == 0 {
                return None;
            }
            *remaining -= 1;
            // Validated when decoded, so this cannot fail.
            read_entry(reader).ok()
        }))
    }

    /// The first entry named `protocol_name`.
    #[must_use]
    pub fn find(&self, protocol_name: &str) -> Option<ProtocolDataEntry<'a>> {
        self.iter()
            .find(|entry| entry.protocol_name == protocol_name)
    }

    /// The first entry, if any.
    #[must_use]
    pub fn first(&self) -> Option<ProtocolDataEntry<'a>> {
        self.iter().next()
    }

    fn content_len(&self) -> usize {
        self.iter().map(|entry| entry_len(&entry)).sum()
    }
}

fn entry_len(entry: &ProtocolDataEntry<'_>) -> usize {
    size::prefixed(entry.protocol_name.len()) + 1 + size::prefixed(entry.data.len())
}

fn read_entry<'a>(reader: &mut Reader<'a>) -> Result<ProtocolDataEntry<'a>, DecodeError> {
    Ok(ProtocolDataEntry {
        protocol_name: reader.read_ascii(PROTOCOL_NAME_SIZE, Alphabet::IA5)?,
        content_type: ContentType(reader.read_u8()?),
        data: reader.read_octets(Size::ANY)?,
    })
}

impl Encode for ProtocolData<'_> {
    fn encoded_len(&self) -> usize {
        size::quantity(self.len()) + self.content_len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        assert!(
            self.len() <= MAX_PROTOCOL_DATA_ENTRIES,
            "too many protocol data entries"
        );
        writer.write_quantity(self.len());
        for entry in self.iter() {
            writer.write_ascii(entry.protocol_name, PROTOCOL_NAME_SIZE, Alphabet::IA5);
            writer.write_u8(entry.content_type.0);
            writer.write_octets(entry.data, Size::ANY);
        }
    }
}

impl<'a> Decode<'a> for ProtocolData<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let count = reader.read_quantity(MAX_PROTOCOL_DATA_ENTRIES)?;
        let start = reader.position();
        for _ in 0..count {
            read_entry(reader)?;
        }
        Ok(Self::Encoded {
            count,
            bytes: reader.consumed_since(start),
        })
    }
}

/// A BTP Error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error<'a> {
    /// A three-character code, as in ILP rejects.
    pub code: &'a str,
    /// The error's name, for example `NotAcceptedError`.
    pub name: &'a str,
    /// When it happened, as a GeneralizedTime string. Not interpreted.
    pub triggered_at: &'a str,
    /// Details, at most [`MAX_ERROR_DATA_LENGTH`] octets.
    pub data: &'a [u8],
    /// Protocol data.
    pub protocol_data: ProtocolData<'a>,
}

/// The body of a BTP packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body<'a> {
    /// A Response.
    Response(ProtocolData<'a>),
    /// An Error.
    Error(Error<'a>),
    /// A Message.
    Message(ProtocolData<'a>),
    /// A Transfer.
    Transfer {
        /// The transferred amount.
        amount: u64,
        /// Protocol data.
        protocol_data: ProtocolData<'a>,
    },
}

/// A BTP packet: type, request ID and body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    /// Correlates responses with requests.
    pub request_id: u32,
    /// The typed body.
    pub body: Body<'a>,
}

impl<'a> Body<'a> {
    /// The protocol data, which every body type carries.
    #[must_use]
    pub const fn protocol_data(&self) -> ProtocolData<'a> {
        match *self {
            Self::Response(data) | Self::Message(data) => data,
            Self::Transfer { protocol_data, .. } => protocol_data,
            Self::Error(error) => error.protocol_data,
        }
    }

    const fn packet_type(&self) -> PacketType {
        match self {
            Self::Response(_) => PacketType::Response,
            Self::Error(_) => PacketType::Error,
            Self::Message(_) => PacketType::Message,
            Self::Transfer { .. } => PacketType::Transfer,
        }
    }

    fn content_len(&self) -> usize {
        match self {
            Self::Response(data) | Self::Message(data) => data.encoded_len(),
            Self::Transfer { protocol_data, .. } => 8 + protocol_data.encoded_len(),
            Self::Error(error) => {
                3 + size::prefixed(error.name.len())
                    + size::prefixed(error.triggered_at.len())
                    + size::prefixed(error.data.len())
                    + error.protocol_data.encoded_len()
            }
        }
    }

    fn encode_content(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Response(data) | Self::Message(data) => data.encode_to(writer),
            Self::Transfer {
                amount,
                protocol_data,
            } => {
                writer.write_u64(*amount);
                protocol_data.encode_to(writer);
            }
            Self::Error(error) => {
                writer.write_fixed_ascii(error.code, 3, Alphabet::IA5);
                writer.write_ascii(error.name, TEXT_SIZE, Alphabet::IA5);
                writer.write_ascii(error.triggered_at, TEXT_SIZE, Alphabet::IA5);
                writer.write_octets(error.data, ERROR_DATA_SIZE);
                error.protocol_data.encode_to(writer);
            }
        }
    }
}

impl Encode for Packet<'_> {
    fn encoded_len(&self) -> usize {
        1 + 4 + size::prefixed(self.body.content_len())
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u8(self.body.packet_type() as u8);
        writer.write_u32(self.request_id);
        writer.write_prefixed(self.body.content_len(), |writer| {
            self.body.encode_content(writer)
        });
    }
}

fn read_body<'a>(
    packet_type: PacketType,
    reader: &mut Reader<'a>,
) -> Result<Body<'a>, DecodeError> {
    Ok(match packet_type {
        PacketType::Response => Body::Response(ProtocolData::decode(reader)?),
        PacketType::Message => Body::Message(ProtocolData::decode(reader)?),
        PacketType::Transfer => Body::Transfer {
            amount: reader.read_u64()?,
            protocol_data: ProtocolData::decode(reader)?,
        },
        PacketType::Error => Body::Error(Error {
            code: reader.read_fixed_ascii(3, Alphabet::IA5)?,
            name: reader.read_ascii(TEXT_SIZE, Alphabet::IA5)?,
            triggered_at: reader.read_ascii(TEXT_SIZE, Alphabet::IA5)?,
            data: reader.read_octets(ERROR_DATA_SIZE)?,
            protocol_data: ProtocolData::decode(reader)?,
        }),
    })
}

impl<'a> Decode<'a> for Packet<'a> {
    type Error = ParseError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, ParseError> {
        let offset = reader.offset();
        let octet = reader.read_u8()?;
        let packet_type = PacketType::from_octet(octet)
            .ok_or(ParseError::new(offset, ParseErrorKind::UnknownType(octet)))?;
        let request_id = reader.read_u32()?;
        let body = reader.read_prefixed(|inner| read_body(packet_type, inner))?;
        Ok(Self { request_id, body })
    }
}

/// The auth token of a BTP authentication request (RFC 23 §"Authentication"):
/// a Message whose first entry is an empty `auth` entry and which carries an
/// `auth_token` entry of type text.
#[must_use]
pub fn auth_token<'a>(packet: &Packet<'a>) -> Option<&'a str> {
    let Body::Message(protocol_data) = packet.body else {
        return None;
    };
    let first = protocol_data.first()?;
    if first.protocol_name != "auth" || !first.data.is_empty() {
        return None;
    }
    let token = protocol_data.find("auth_token")?;
    if token.content_type != ContentType::TEXT_PLAIN_UTF8 {
        return None;
    }
    core::str::from_utf8(token.data).ok()
}
