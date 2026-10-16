//! STREAM frames (RFC 29 §5.3).

use dassie_ilp::{Address, MAX_ADDRESS_LENGTH};
use dassie_oer::{Alphabet, DecodeError, DecodeErrorKind, Reader, Size, Writer, size};

/// A STREAM error code (RFC 29 §5.4). Unknown codes pass through: the
/// fixtures use 255.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ErrorCode(pub u8);

impl ErrorCode {
    /// Closed normally.
    pub const NO_ERROR: Self = Self(0x01);
    /// Something unexpected went wrong.
    pub const INTERNAL_ERROR: Self = Self(0x02);
    /// The endpoint is temporarily overloaded.
    pub const ENDPOINT_BUSY: Self = Self(0x03);
    /// The peer sent more than the advertised limits allowed.
    pub const FLOW_CONTROL_ERROR: Self = Self(0x04);
    /// A stream ID was invalid or above the advertised maximum.
    pub const STREAM_ID_ERROR: Self = Self(0x05);
    /// A frame arrived for a stream in the wrong state.
    pub const STREAM_STATE_ERROR: Self = Self(0x06);
    /// A frame was malformed.
    pub const FRAME_FORMAT_ERROR: Self = Self(0x07);
    /// The peer violated the protocol.
    pub const PROTOCOL_VIOLATION: Self = Self(0x08);
    /// The application closed with an error.
    pub const APPLICATION_ERROR: Self = Self(0x09);
}

/// Frame type octets (RFC 29 §5.3).
pub mod frame_type {
    /// `ConnectionClose`.
    pub const CONNECTION_CLOSE: u8 = 0x01;
    /// `ConnectionNewAddress`.
    pub const CONNECTION_NEW_ADDRESS: u8 = 0x02;
    /// `ConnectionMaxData`.
    pub const CONNECTION_MAX_DATA: u8 = 0x03;
    /// `ConnectionDataBlocked`.
    pub const CONNECTION_DATA_BLOCKED: u8 = 0x04;
    /// `ConnectionMaxStreamId`.
    pub const CONNECTION_MAX_STREAM_ID: u8 = 0x05;
    /// `ConnectionStreamIdBlocked`.
    pub const CONNECTION_STREAM_ID_BLOCKED: u8 = 0x06;
    /// `ConnectionAssetDetails`.
    pub const CONNECTION_ASSET_DETAILS: u8 = 0x07;
    /// `StreamClose`.
    pub const STREAM_CLOSE: u8 = 0x10;
    /// `StreamMoney`.
    pub const STREAM_MONEY: u8 = 0x11;
    /// `StreamMaxMoney`.
    pub const STREAM_MAX_MONEY: u8 = 0x12;
    /// `StreamMoneyBlocked`.
    pub const STREAM_MONEY_BLOCKED: u8 = 0x13;
    /// `StreamData`.
    pub const STREAM_DATA: u8 = 0x14;
    /// `StreamMaxData`.
    pub const STREAM_MAX_DATA: u8 = 0x15;
    /// `StreamDataBlocked`.
    pub const STREAM_DATA_BLOCKED: u8 = 0x16;
    /// `StreamReceipt`.
    pub const STREAM_RECEIPT: u8 = 0x17;
}

const ADDRESS_SIZE: Size = Size::range(0, MAX_ADDRESS_LENGTH);

/// A STREAM frame, borrowing its strings and bytes from the packet.
///
/// Integer fields are `u64`. RFC 29 encodes them as unbounded integers and
/// asks receivers to treat anything larger as the maximum, so decoding
/// saturates (the original kept the exact big integer, spec 4 §11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    /// The sender closes the connection.
    ConnectionClose {
        /// Why.
        code: ErrorCode,
        /// Details for humans.
        message: &'a str,
    },
    /// The sender's ILP address, which the receiver sends packets to. RFC 29
    /// allows it to be empty; the original could not parse that.
    ConnectionNewAddress {
        /// The address, if the sender has one.
        source_account: Option<Address<'a>>,
    },
    /// The total number of bytes, across streams, the sender will accept.
    ConnectionMaxData {
        /// The highest connection-level offset accepted.
        max_offset: u64,
    },
    /// The sender has more data than the receiver allows.
    ConnectionDataBlocked {
        /// The connection-level offset it wants to reach.
        max_offset: u64,
    },
    /// The highest stream ID the sender lets the receiver open.
    ConnectionMaxStreamId {
        /// The limit.
        max_stream_id: u64,
    },
    /// The sender wants to open more streams than allowed.
    ConnectionStreamIdBlocked {
        /// The ID it wants to open.
        max_stream_id: u64,
    },
    /// The sender's asset.
    ConnectionAssetDetails {
        /// Asset code, for example `USD`.
        asset_code: &'a str,
        /// Decimal places.
        asset_scale: u8,
    },
    /// The sender closes a stream.
    StreamClose {
        /// The stream.
        stream_id: u64,
        /// Why.
        code: ErrorCode,
        /// Details for humans.
        message: &'a str,
    },
    /// The packet's amount is split among streams in proportion to shares.
    StreamMoney {
        /// The stream.
        stream_id: u64,
        /// This stream's share of the packet's amount.
        shares: u64,
    },
    /// How much a stream will accept.
    StreamMaxMoney {
        /// The stream.
        stream_id: u64,
        /// The most the stream will receive in total.
        receive_max: u64,
        /// What it has received so far.
        total_received: u64,
    },
    /// The sender wants to send more than the receiver allows.
    StreamMoneyBlocked {
        /// The stream.
        stream_id: u64,
        /// The total the sender wants to send.
        send_max: u64,
        /// What it has sent so far.
        total_sent: u64,
    },
    /// Stream data at an offset.
    StreamData {
        /// The stream.
        stream_id: u64,
        /// Offset of the first byte.
        offset: u64,
        /// The bytes.
        data: &'a [u8],
    },
    /// How many bytes a stream will accept.
    StreamMaxData {
        /// The stream.
        stream_id: u64,
        /// The highest offset accepted.
        max_offset: u64,
    },
    /// The sender has more data for a stream than allowed.
    StreamDataBlocked {
        /// The stream.
        stream_id: u64,
        /// The offset it wants to reach.
        max_offset: u64,
    },
    /// A receipt for money received on a stream (RFC 39).
    StreamReceipt {
        /// The stream.
        stream_id: u64,
        /// The receipt bytes.
        receipt: &'a [u8],
    },
}

impl Frame<'_> {
    /// The frame's type octet.
    #[must_use]
    pub const fn frame_type(&self) -> u8 {
        use crate::frame::frame_type as t;
        match self {
            Self::ConnectionClose { .. } => t::CONNECTION_CLOSE,
            Self::ConnectionNewAddress { .. } => t::CONNECTION_NEW_ADDRESS,
            Self::ConnectionMaxData { .. } => t::CONNECTION_MAX_DATA,
            Self::ConnectionDataBlocked { .. } => t::CONNECTION_DATA_BLOCKED,
            Self::ConnectionMaxStreamId { .. } => t::CONNECTION_MAX_STREAM_ID,
            Self::ConnectionStreamIdBlocked { .. } => t::CONNECTION_STREAM_ID_BLOCKED,
            Self::ConnectionAssetDetails { .. } => t::CONNECTION_ASSET_DETAILS,
            Self::StreamClose { .. } => t::STREAM_CLOSE,
            Self::StreamMoney { .. } => t::STREAM_MONEY,
            Self::StreamMaxMoney { .. } => t::STREAM_MAX_MONEY,
            Self::StreamMoneyBlocked { .. } => t::STREAM_MONEY_BLOCKED,
            Self::StreamData { .. } => t::STREAM_DATA,
            Self::StreamMaxData { .. } => t::STREAM_MAX_DATA,
            Self::StreamDataBlocked { .. } => t::STREAM_DATA_BLOCKED,
            Self::StreamReceipt { .. } => t::STREAM_RECEIPT,
        }
    }

    /// Length of the frame's content, without type and length determinant.
    #[must_use]
    pub fn content_len(&self) -> usize {
        use size::{prefixed, var_uint};
        match *self {
            Self::ConnectionClose { message, .. } => 1 + prefixed(message.len()),
            Self::ConnectionNewAddress { source_account } => {
                prefixed(source_account.map_or(0, |address| address.as_str().len()))
            }
            Self::ConnectionMaxData { max_offset: value }
            | Self::ConnectionDataBlocked { max_offset: value }
            | Self::ConnectionMaxStreamId {
                max_stream_id: value,
            }
            | Self::ConnectionStreamIdBlocked {
                max_stream_id: value,
            } => var_uint(value),
            Self::ConnectionAssetDetails { asset_code, .. } => prefixed(asset_code.len()) + 1,
            Self::StreamClose {
                stream_id, message, ..
            } => var_uint(stream_id) + 1 + prefixed(message.len()),
            Self::StreamMoney { stream_id, shares } => var_uint(stream_id) + var_uint(shares),
            Self::StreamMaxMoney {
                stream_id,
                receive_max: first,
                total_received: second,
            }
            | Self::StreamMoneyBlocked {
                stream_id,
                send_max: first,
                total_sent: second,
            } => var_uint(stream_id) + var_uint(first) + var_uint(second),
            Self::StreamData {
                stream_id,
                offset,
                data,
            } => var_uint(stream_id) + var_uint(offset) + prefixed(data.len()),
            Self::StreamMaxData {
                stream_id,
                max_offset,
            }
            | Self::StreamDataBlocked {
                stream_id,
                max_offset,
            } => var_uint(stream_id) + var_uint(max_offset),
            Self::StreamReceipt { stream_id, receipt } => {
                var_uint(stream_id) + prefixed(receipt.len())
            }
        }
    }

    /// Length of the whole frame: type, length determinant and content.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        1 + size::prefixed(self.content_len())
    }

    /// Writes the whole frame.
    pub fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u8(self.frame_type());
        writer.write_prefixed(self.content_len(), |writer| self.encode_content(writer));
    }

    fn encode_content(&self, writer: &mut Writer<'_>) {
        match *self {
            Self::ConnectionClose { code, message } => {
                writer.write_u8(code.0);
                writer.write_utf8(message, Size::ANY);
            }
            Self::ConnectionNewAddress { source_account } => {
                let text = source_account.map_or("", |address| address.as_str());
                writer.write_ascii(text, ADDRESS_SIZE, Alphabet::IA5);
            }
            Self::ConnectionMaxData { max_offset: value }
            | Self::ConnectionDataBlocked { max_offset: value }
            | Self::ConnectionMaxStreamId {
                max_stream_id: value,
            }
            | Self::ConnectionStreamIdBlocked {
                max_stream_id: value,
            } => writer.write_var_uint(value),
            Self::ConnectionAssetDetails {
                asset_code,
                asset_scale,
            } => {
                writer.write_utf8(asset_code, Size::ANY);
                writer.write_u8(asset_scale);
            }
            Self::StreamClose {
                stream_id,
                code,
                message,
            } => {
                writer.write_var_uint(stream_id);
                writer.write_u8(code.0);
                writer.write_utf8(message, Size::ANY);
            }
            _ => self.encode_stream_content(writer),
        }
    }

    // Panics only on a broken invariant: `encode_content` takes the rest.
    #[allow(clippy::unreachable)]
    fn encode_stream_content(&self, writer: &mut Writer<'_>) {
        match *self {
            Self::StreamMoney { stream_id, shares } => {
                writer.write_var_uint(stream_id);
                writer.write_var_uint(shares);
            }
            Self::StreamMaxMoney {
                stream_id,
                receive_max: first,
                total_received: second,
            }
            | Self::StreamMoneyBlocked {
                stream_id,
                send_max: first,
                total_sent: second,
            } => {
                writer.write_var_uint(stream_id);
                writer.write_var_uint(first);
                writer.write_var_uint(second);
            }
            Self::StreamData {
                stream_id,
                offset,
                data,
            } => {
                writer.write_var_uint(stream_id);
                writer.write_var_uint(offset);
                writer.write_octets(data, Size::ANY);
            }
            Self::StreamMaxData {
                stream_id,
                max_offset,
            }
            | Self::StreamDataBlocked {
                stream_id,
                max_offset,
            } => {
                writer.write_var_uint(stream_id);
                writer.write_var_uint(max_offset);
            }
            Self::StreamReceipt { stream_id, receipt } => {
                writer.write_var_uint(stream_id);
                writer.write_octets(receipt, Size::ANY);
            }
            _ => unreachable!("connection frames are encoded by encode_content"),
        }
    }

    /// Reads one frame. Returns `None` for a frame type this implementation
    /// does not know, after skipping it: RFC 29 §5.3 asks receivers to ignore
    /// unknown frames, which the original rejected (ILP-11).
    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Option<Frame<'a>>, DecodeError> {
        let frame_type = reader.read_u8()?;
        // Bytes after a known frame's fields are ignored, as upstream does,
        // so that frames can gain fields without breaking older receivers.
        let content = reader.read_octets(Size::ANY)?;
        decode_content(frame_type, &mut Reader::new(content))
    }
}

fn decode_content<'a>(kind: u8, reader: &mut Reader<'a>) -> Result<Option<Frame<'a>>, DecodeError> {
    use crate::frame::frame_type as t;
    let frame = match kind {
        t::CONNECTION_CLOSE => Frame::ConnectionClose {
            code: ErrorCode(reader.read_u8()?),
            message: reader.read_utf8(Size::ANY)?,
        },
        t::CONNECTION_NEW_ADDRESS => {
            let offset = reader.offset();
            let text = reader.read_ascii(ADDRESS_SIZE, Alphabet::IA5)?;
            let source_account = if text.is_empty() {
                None
            } else {
                let address = Address::parse(text)
                    .map_err(|_| DecodeError::new(offset, DecodeErrorKind::UnexpectedValue))?;
                Some(address)
            };
            Frame::ConnectionNewAddress { source_account }
        }
        t::CONNECTION_MAX_DATA => Frame::ConnectionMaxData {
            max_offset: reader.read_var_uint_saturating()?,
        },
        t::CONNECTION_DATA_BLOCKED => Frame::ConnectionDataBlocked {
            max_offset: reader.read_var_uint_saturating()?,
        },
        t::CONNECTION_MAX_STREAM_ID => Frame::ConnectionMaxStreamId {
            max_stream_id: reader.read_var_uint_saturating()?,
        },
        t::CONNECTION_STREAM_ID_BLOCKED => Frame::ConnectionStreamIdBlocked {
            max_stream_id: reader.read_var_uint_saturating()?,
        },
        t::CONNECTION_ASSET_DETAILS => Frame::ConnectionAssetDetails {
            asset_code: reader.read_utf8(Size::ANY)?,
            asset_scale: reader.read_u8()?,
        },
        t::STREAM_CLOSE => Frame::StreamClose {
            stream_id: reader.read_var_uint_saturating()?,
            code: ErrorCode(reader.read_u8()?),
            message: reader.read_utf8(Size::ANY)?,
        },
        _ => return decode_stream_content(kind, reader),
    };
    Ok(Some(frame))
}

fn decode_stream_content<'a>(
    kind: u8,
    reader: &mut Reader<'a>,
) -> Result<Option<Frame<'a>>, DecodeError> {
    use crate::frame::frame_type as t;
    let frame = match kind {
        t::STREAM_MONEY => Frame::StreamMoney {
            stream_id: reader.read_var_uint_saturating()?,
            shares: reader.read_var_uint_saturating()?,
        },
        t::STREAM_MAX_MONEY => Frame::StreamMaxMoney {
            stream_id: reader.read_var_uint_saturating()?,
            receive_max: reader.read_var_uint_saturating()?,
            total_received: reader.read_var_uint_saturating()?,
        },
        t::STREAM_MONEY_BLOCKED => Frame::StreamMoneyBlocked {
            stream_id: reader.read_var_uint_saturating()?,
            send_max: reader.read_var_uint_saturating()?,
            total_sent: reader.read_var_uint_saturating()?,
        },
        t::STREAM_DATA => Frame::StreamData {
            stream_id: reader.read_var_uint_saturating()?,
            offset: reader.read_var_uint_saturating()?,
            data: reader.read_octets(Size::ANY)?,
        },
        t::STREAM_MAX_DATA => Frame::StreamMaxData {
            stream_id: reader.read_var_uint_saturating()?,
            max_offset: reader.read_var_uint_saturating()?,
        },
        t::STREAM_DATA_BLOCKED => Frame::StreamDataBlocked {
            stream_id: reader.read_var_uint_saturating()?,
            max_offset: reader.read_var_uint_saturating()?,
        },
        t::STREAM_RECEIPT => Frame::StreamReceipt {
            stream_id: reader.read_var_uint_saturating()?,
            receipt: reader.read_octets(Size::ANY)?,
        },
        _ => {
            // Unknown frame: consume its content and report nothing.
            reader.read_bytes(reader.remaining())?;
            return Ok(None);
        }
    };
    Ok(Some(frame))
}
