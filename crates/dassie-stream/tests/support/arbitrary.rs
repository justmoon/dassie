//! Proptest strategies for hostile input: encoded frames, packets and
//! tokens a remote could send, and ways to damage them. Random bytes rarely
//! get past a header, so these start from valid encodings that reach the
//! bodies of the decoders.

use dassie_ilp::Address;
use dassie_oer::{Size, Writer, size};
use dassie_stream::crypto::{Cipher, OVERHEAD};
use dassie_stream::packet::{self, PacketType};
use dassie_stream::receipt::RECEIPT_LENGTH;
use dassie_stream::token::TOKEN_NONCE_LENGTH;
use dassie_stream::{ErrorCode, Frame};
use proptest::prelude::*;

/// A value that hits streams, offsets and limits a connection has, or
/// anything at all.
fn number() -> impl Strategy<Value = u64> {
    prop_oneof![3 => 0u64..16, 2 => 0u64..70_000, 1 => any::<u64>()]
}

/// Mostly the first few streams a client opens, which a server lets it
/// open. Other IDs end the connection, so that the frames after them would
/// otherwise rarely be applied.
fn stream_id() -> impl Strategy<Value = u64> {
    prop_oneof![6 => (0u64..4).prop_map(|n| 2 * n + 1), 1 => number()]
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof!["[A-Z]{0,4}", any::<String>()]
}

/// Bytes, sometimes as long as a receipt.
fn bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        2 => prop::collection::vec(any::<u8>(), 0..64),
        1 => prop::collection::vec(any::<u8>(), RECEIPT_LENGTH),
    ]
}

fn address() -> impl Strategy<Value = Option<String>> {
    prop::option::of("(test|g)(\\.[a-zA-Z0-9_~-]{1,8}){1,3}")
}

/// Appends `bytes` as a length-prefixed octet string.
pub fn push_octets(buffer: &mut Vec<u8>, bytes: &[u8]) {
    let start = buffer.len();
    buffer.resize(start + size::prefixed(bytes.len()), 0);
    Writer::new(&mut buffer[start..]).write_octets(bytes, Size::ANY);
}

/// One encoded frame of any type, known or not.
pub fn frame() -> impl Strategy<Value = Vec<u8>> {
    let numbers = (stream_id(), number(), number());
    (0u8..16, numbers, any::<u8>(), text(), bytes(), address()).prop_map(
        |(kind, (a, b, c), octet, text, bytes, address)| {
            let source_account = address.as_deref().map(|text| Address::parse(text).unwrap());
            let code = ErrorCode(octet);
            let message = text.as_str();
            let frame = match kind {
                0 => Frame::ConnectionClose { code, message },
                1 => Frame::ConnectionNewAddress { source_account },
                2 => Frame::ConnectionMaxData { max_offset: a },
                3 => Frame::ConnectionDataBlocked { max_offset: a },
                4 => Frame::ConnectionMaxStreamId { max_stream_id: a },
                5 => Frame::ConnectionStreamIdBlocked { max_stream_id: a },
                6 => Frame::ConnectionAssetDetails {
                    asset_code: message,
                    asset_scale: octet,
                },
                7 => Frame::StreamClose {
                    stream_id: a,
                    code,
                    message,
                },
                8 => Frame::StreamMoney {
                    stream_id: a,
                    shares: b,
                },
                9 => Frame::StreamMaxMoney {
                    stream_id: a,
                    receive_max: b,
                    total_received: c,
                },
                10 => Frame::StreamMoneyBlocked {
                    stream_id: a,
                    send_max: b,
                    total_sent: c,
                },
                11 => Frame::StreamData {
                    stream_id: a,
                    offset: b,
                    data: &bytes,
                },
                12 => Frame::StreamMaxData {
                    stream_id: a,
                    max_offset: b,
                },
                13 => Frame::StreamDataBlocked {
                    stream_id: a,
                    max_offset: b,
                },
                14 => Frame::StreamReceipt {
                    stream_id: a,
                    receipt: &bytes,
                },
                _ => {
                    // A type no version defines: 0x18 and up.
                    let mut buffer = vec![0x18 + octet % 0xe8];
                    push_octets(&mut buffer, &bytes);
                    return buffer;
                }
            };
            let mut buffer = vec![0; frame.encoded_len()];
            frame.encode_to(&mut Writer::new(&mut buffer));
            buffer
        },
    )
}

/// An encoded STREAM packet carrying the encoded `frames`.
pub fn packet(
    sequence: u64,
    packet_type: PacketType,
    prepare_amount: u64,
    frames: &[Vec<u8>],
) -> Vec<u8> {
    let mut buffer = vec![0; packet::header_len(sequence, prepare_amount, frames.len())];
    let mut writer = Writer::new(&mut buffer);
    packet::encode_header(
        &mut writer,
        sequence,
        packet_type,
        prepare_amount,
        frames.len(),
    );
    for frame in frames {
        buffer.extend_from_slice(frame);
    }
    buffer
}

/// One way to damage an encoding. Positions wrap around its length.
#[derive(Clone, Debug)]
pub enum Mutation {
    /// Overwrite a byte.
    Set(usize, u8),
    /// Cut it short.
    Truncate(usize),
    /// Insert bytes, at the end to extend it.
    Insert(usize, Vec<u8>),
    /// Remove a byte.
    Remove(usize),
}

/// Nothing half the time, so that intact encodings reach deep too, or a
/// few mutations.
pub fn mutations() -> impl Strategy<Value = Vec<Mutation>> {
    let mutation = prop_oneof![
        3 => (any::<usize>(), any::<u8>()).prop_map(|(at, value)| Mutation::Set(at, value)),
        1 => any::<usize>().prop_map(Mutation::Truncate),
        1 => (any::<usize>(), prop::collection::vec(any::<u8>(), 1..8))
            .prop_map(|(at, bytes)| Mutation::Insert(at, bytes)),
        1 => any::<usize>().prop_map(Mutation::Remove),
    ];
    prop_oneof![
        1 => Just(Vec::new()),
        1 => prop::collection::vec(mutation, 1..4),
    ]
}

/// Applies `mutations` to `bytes` in order.
pub fn damage(bytes: &mut Vec<u8>, mutations: &[Mutation]) {
    for mutation in mutations {
        let length = bytes.len();
        match mutation {
            Mutation::Set(at, value) if length > 0 => bytes[at % length] = *value,
            Mutation::Truncate(at) => bytes.truncate(at % (length + 1)),
            Mutation::Insert(at, inserted) => {
                let at = at % (length + 1);
                bytes.splice(at..at, inserted.iter().copied());
            }
            Mutation::Remove(at) if length > 0 => {
                bytes.remove(at % length);
            }
            _ => {}
        }
    }
}

/// What a remote encrypts: a STREAM packet, possibly damaged, or bytes.
#[derive(Clone, Debug)]
pub enum Payload {
    /// Arbitrary bytes.
    Bytes(Vec<u8>),
    /// A packet with these frames, then damaged.
    Packet {
        prepare_amount: u64,
        frames: Vec<Vec<u8>>,
        mutations: Vec<Mutation>,
    },
}

/// Mostly packets, whose frames are what the receiver acts on. Their
/// minimum amount is mostly zero, so that more of them can be fulfilled.
pub fn payload() -> impl Strategy<Value = Payload> {
    let prepare_amount = prop_oneof![3 => Just(0), 1 => number()];
    prop_oneof![
        1 => prop::collection::vec(any::<u8>(), 0..200).prop_map(Payload::Bytes),
        4 => (prepare_amount, prop::collection::vec(frame(), 0..8), mutations()).prop_map(
            |(prepare_amount, frames, mutations)| Payload::Packet {
                prepare_amount,
                frames,
                mutations,
            }
        ),
    ]
}

impl Payload {
    /// The plaintext, as a packet with `sequence` and `packet_type`.
    pub fn plaintext(&self, sequence: u64, packet_type: PacketType) -> Vec<u8> {
        match self {
            Self::Bytes(bytes) => bytes.clone(),
            Self::Packet {
                prepare_amount,
                frames,
                mutations,
            } => {
                let mut bytes = packet(sequence, packet_type, *prepare_amount, frames);
                damage(&mut bytes, mutations);
                bytes
            }
        }
    }
}

/// The plaintext of a server token: nonce, tag, receipt nonce and secret,
/// with tags beyond the limit and receipt fields of any length, then
/// damaged.
pub fn token_plaintext() -> impl Strategy<Value = Vec<u8>> {
    let receipt = prop_oneof![
        Just((Vec::new(), Vec::new())),
        (
            prop::collection::vec(any::<u8>(), 16),
            prop::collection::vec(any::<u8>(), 32)
        ),
        (
            prop::collection::vec(any::<u8>(), 0..40),
            prop::collection::vec(any::<u8>(), 0..40)
        ),
    ];
    (
        any::<[u8; TOKEN_NONCE_LENGTH]>(),
        prop::collection::vec(any::<u8>(), 0..140),
        receipt,
        mutations(),
    )
        .prop_map(|(nonce, tag, (receipt_nonce, secret), mutations)| {
            let mut bytes = nonce.to_vec();
            push_octets(&mut bytes, &tag);
            push_octets(&mut bytes, &receipt_nonce);
            push_octets(&mut bytes, &secret);
            damage(&mut bytes, &mutations);
            bytes
        })
}

/// Encrypts `plaintext` under `cipher`. A fixed nonce, so only for tests.
pub fn seal(cipher: &Cipher, plaintext: &[u8]) -> Vec<u8> {
    let mut buffer = vec![0; OVERHEAD + plaintext.len()];
    buffer[OVERHEAD..].copy_from_slice(plaintext);
    let length = cipher.encrypt_with_iv([7; 12], plaintext.len(), &mut buffer);
    buffer.truncate(length);
    buffer
}
