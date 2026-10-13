//! STREAM packets (RFC 29 §5.1): the plaintext inside ILP packet data.

use dassie_oer::{DecodeError, DecodeErrorKind, Reader, Writer, size};

use crate::frame::Frame;

/// The only STREAM version (RFC 29 §5.1).
pub const VERSION: u8 = 1;

/// Most frames a packet may carry. RFC 29 sets no limit; this one is far
/// above what any sender needs and keeps decoding bounded (D16).
pub const MAX_FRAMES: usize = 512;

/// The ILP packet type a STREAM packet says it travels on. A response that
/// claims a different type than the packet carrying it is not trusted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    /// Carried in a Prepare.
    Prepare,
    /// Carried in a Fulfill.
    Fulfill,
    /// Carried in a Reject.
    Reject,
}

impl PacketType {
    /// The ILP type octet.
    #[must_use]
    pub const fn octet(self) -> u8 {
        match self {
            Self::Prepare => dassie_ilp::TYPE_PREPARE,
            Self::Fulfill => dassie_ilp::TYPE_FULFILL,
            Self::Reject => dassie_ilp::TYPE_REJECT,
        }
    }

    const fn from_octet(octet: u8) -> Option<Self> {
        match octet {
            dassie_ilp::TYPE_PREPARE => Some(Self::Prepare),
            dassie_ilp::TYPE_FULFILL => Some(Self::Fulfill),
            dassie_ilp::TYPE_REJECT => Some(Self::Reject),
            _ => None,
        }
    }
}

/// The frames of a packet: a list to encode, or the validated encoding of
/// decoded frames, iterated without allocating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frames<'a> {
    /// Frames to encode.
    List(&'a [Frame<'a>]),
    /// Decoded frames: how many there are (including unknown ones, which
    /// iteration skips) and their encodings.
    Encoded {
        /// Number of encoded frames.
        count: usize,
        /// The frames' encodings, already validated.
        bytes: &'a [u8],
    },
}

impl<'a> Frames<'a> {
    /// The known frames in order. Unknown frames are skipped.
    pub fn iter(&self) -> impl Iterator<Item = Frame<'a>> + 'a {
        let (list, encoded) = match *self {
            Self::List(list) => (list, None),
            Self::Encoded { count, bytes } => (&[][..], Some((count, Reader::new(bytes)))),
        };
        let mut encoded = encoded;
        list.iter().copied().chain(core::iter::from_fn(move || {
            let (remaining, reader) = encoded.as_mut()?;
            while *remaining > 0 {
                *remaining -= 1;
                // Validated when decoded, so this cannot fail.
                match Frame::decode(reader) {
                    Ok(Some(frame)) => return Some(frame),
                    Ok(None) => {}
                    Err(_) => return None,
                }
            }
            None
        }))
    }

    fn count(&self) -> usize {
        match self {
            Self::List(list) => list.len(),
            Self::Encoded { count, .. } => *count,
        }
    }

    fn content_len(&self) -> usize {
        match self {
            Self::List(list) => list.iter().map(Frame::encoded_len).sum(),
            Self::Encoded { bytes, .. } => bytes.len(),
        }
    }
}

/// A STREAM packet (RFC 29 §5.1), the plaintext inside an ILP packet's
/// data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    /// Identifies a request and its response.
    pub sequence: u64,
    /// The ILP packet type this packet travels on.
    pub packet_type: PacketType,
    /// In a request, the least the receiver must get to accept the packet;
    /// in a response, what the receiver got.
    pub prepare_amount: u64,
    /// The frames.
    pub frames: Frames<'a>,
}

/// Length of a packet's fields before its frames.
#[must_use]
pub const fn header_len(sequence: u64, prepare_amount: u64, frame_count: usize) -> usize {
    2 + size::var_uint(sequence) + size::var_uint(prepare_amount) + size::quantity(frame_count)
}

/// Writes a packet's fields before its frames, for callers that write the
/// frames themselves straight from their state.
pub fn encode_header(
    writer: &mut Writer<'_>,
    sequence: u64,
    packet_type: PacketType,
    prepare_amount: u64,
    frame_count: usize,
) {
    writer.write_u8(VERSION);
    writer.write_u8(packet_type.octet());
    writer.write_var_uint(sequence);
    writer.write_var_uint(prepare_amount);
    writer.write_quantity(frame_count);
}

impl<'a> Packet<'a> {
    /// Length of the encoding.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        header_len(self.sequence, self.prepare_amount, self.frames.count())
            + self.frames.content_len()
    }

    /// Writes the packet.
    pub fn encode_to(&self, writer: &mut Writer<'_>) {
        encode_header(
            writer,
            self.sequence,
            self.packet_type,
            self.prepare_amount,
            self.frames.count(),
        );
        match self.frames {
            Frames::List(list) => {
                for frame in list {
                    frame.encode_to(writer);
                }
            }
            Frames::Encoded { bytes, .. } => writer.write_bytes(bytes),
        }
    }

    /// Parses a decrypted packet.
    ///
    /// Bytes after the frames are ignored, as RFC 29 §5.1 asks so that
    /// senders can pad packets; the original rejected them (ILP-11). Unknown
    /// frames are validated as frames but otherwise skipped.
    pub fn parse(plaintext: &'a [u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(plaintext);
        let version_offset = reader.offset();
        if reader.read_u8()? != VERSION {
            return Err(DecodeError::new(
                version_offset,
                DecodeErrorKind::UnexpectedValue,
            ));
        }
        let type_offset = reader.offset();
        let packet_type = PacketType::from_octet(reader.read_u8()?).ok_or(DecodeError::new(
            type_offset,
            DecodeErrorKind::UnexpectedValue,
        ))?;
        let sequence = reader.read_var_uint_saturating()?;
        let prepare_amount = reader.read_var_uint_saturating()?;
        let count = reader.read_quantity(MAX_FRAMES)?;
        let frames_start = reader.position();
        for _ in 0..count {
            Frame::decode(&mut reader)?;
        }
        Ok(Self {
            sequence,
            packet_type,
            prepare_amount,
            frames: Frames::Encoded {
                count,
                bytes: reader.consumed_since(frames_start),
            },
        })
    }
}
