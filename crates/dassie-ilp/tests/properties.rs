//! Properties: parsing never panics, accepted input re-encodes identically,
//! and well-formed packets round-trip.
//!
//! Random bytes rarely get past a type octet and a length prefix, so most
//! decoders are also fed valid encodings that are then damaged: octets
//! changed, inserted or removed, the end cut off or extended. That reaches
//! the bodies, where hostile input does its work
//! (spec/decisions/tigerstyle.md).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use dassie_ilp::btp::{self, Body, ContentType, PacketType, ProtocolData, ProtocolDataEntry};
use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode, Fulfill, Packet, Prepare, Reject, Response, Timestamp,
    ildcp,
};
use dassie_oer::{Decode, Encode, Reader, Writer, size};
use proptest::prelude::*;

fn segment() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_~-]{1,12}"
}

fn address() -> impl Strategy<Value = String> {
    (
        prop::sample::select(vec!["g", "test", "example", "peer", "private"]),
        prop::collection::vec(segment(), 1..5),
    )
        .prop_map(|(scheme, segments)| format!("{scheme}.{}", segments.join(".")))
}

/// Any 7-bit text, the IA5String alphabet.
fn ia5(max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(0u8..0x80, 0..=max).prop_map(|bytes| String::from_utf8(bytes).unwrap())
}

/// Short data, so that damage lands in the headers about as often as in it.
fn data() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..40)
}

/// One way to damage an encoding. Positions wrap around its length.
#[derive(Clone, Debug)]
enum Mutation {
    Change(usize, u8),
    Insert(usize, u8),
    Remove(usize),
    Truncate(usize),
    // Cuts off the last few octets, which ends the last field early.
    CutEnd(usize),
    Extend(Vec<u8>),
}

fn mutations() -> impl Strategy<Value = Vec<Mutation>> {
    let mutation = prop_oneof![
        3 => (any::<usize>(), any::<u8>()).prop_map(|(at, value)| Mutation::Change(at, value)),
        1 => (any::<usize>(), any::<u8>()).prop_map(|(at, value)| Mutation::Insert(at, value)),
        1 => any::<usize>().prop_map(Mutation::Remove),
        1 => any::<usize>().prop_map(Mutation::Truncate),
        1 => (1usize..=16).prop_map(Mutation::CutEnd),
        1 => prop::collection::vec(any::<u8>(), 1..8).prop_map(Mutation::Extend),
    ];
    prop::collection::vec(mutation, 1..4)
}

fn mutate(mut encoded: Vec<u8>, mutations: &[Mutation]) -> Vec<u8> {
    for mutation in mutations {
        let length = encoded.len();
        match mutation {
            Mutation::Change(at, value) if length > 0 => encoded[at % length] = *value,
            Mutation::Insert(at, value) => encoded.insert(at % (length + 1), *value),
            Mutation::Remove(at) if length > 0 => {
                encoded.remove(at % length);
            }
            Mutation::Truncate(at) => encoded.truncate(at % (length + 1)),
            Mutation::CutEnd(count) => encoded.truncate(length.saturating_sub(*count)),
            Mutation::Extend(bytes) => encoded.extend_from_slice(bytes),
            Mutation::Change(..) | Mutation::Remove(_) => {}
        }
    }
    encoded
}

/// Damage to a packet, either to its whole encoding or to the content of
/// its outer length prefix. Damage inside the content mostly breaks that
/// prefix, so in the second case the prefix is rewritten to fit and the
/// damage reaches the body decoder.
#[derive(Clone, Debug)]
struct Damage {
    inside: bool,
    mutations: Vec<Mutation>,
}

fn damage() -> impl Strategy<Value = Damage> {
    (any::<bool>(), mutations()).prop_map(|(inside, mutations)| Damage { inside, mutations })
}

/// Applies `damage` to `encoded`, whose outer length prefix follows
/// `header` octets.
fn apply(encoded: Vec<u8>, header: usize, damage: &Damage) -> Vec<u8> {
    if !damage.inside {
        return mutate(encoded, &damage.mutations);
    }
    let mut reader = Reader::new(&encoded[header..]);
    reader.read_length().unwrap();
    let content = mutate(
        encoded[header + reader.position()..].to_vec(),
        &damage.mutations,
    );
    let mut prefix = vec![0; size::length_determinant(content.len())];
    Writer::new(&mut prefix).write_length(content.len());
    [&encoded[..header], &prefix, &content].concat()
}

/// A reject code: often F08, whose data [`AmountTooLarge`] reads.
fn error_code() -> impl Strategy<Value = ErrorCode> {
    prop_oneof![
        Just(ErrorCode::F08_AMOUNT_TOO_LARGE),
        prop::array::uniform3(0u8..0x80).prop_map(|code| ErrorCode::new(code).unwrap()),
    ]
}

/// Reject data: often well-formed F08 data.
fn reject_data() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        (any::<u64>(), any::<u64>()).prop_map(|(received_amount, maximum_amount)| {
            AmountTooLarge {
                received_amount,
                maximum_amount,
            }
            .to_bytes()
            .to_vec()
        }),
        data(),
    ]
}

/// A protocol data entry, owned until a packet borrows it.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    content_type: u8,
    data: Vec<u8>,
}

fn entry() -> impl Strategy<Value = Entry> {
    (
        prop_oneof![
            prop::sample::select(vec!["ilp", "auth", "auth_token"]).prop_map(String::from),
            ia5(12),
        ],
        prop_oneof![0u8..3, any::<u8>()],
        data(),
    )
        .prop_map(|(name, content_type, data)| Entry {
            name,
            content_type,
            data,
        })
}

/// Protocol data entries shaped like an authentication request, so that
/// [`btp::auth_token`] gets past its checks.
fn auth_entries() -> impl Strategy<Value = Vec<Entry>> {
    let content_type = prop_oneof![3 => Just(ContentType::TEXT_PLAIN_UTF8.0), 1 => any::<u8>()];
    (
        ".{0,16}",
        content_type,
        prop::collection::vec(entry(), 0..3),
    )
        .prop_map(|(token, content_type, rest)| {
            let mut entries = vec![
                Entry {
                    name: "auth".into(),
                    content_type: ContentType::OCTET_STREAM.0,
                    data: Vec::new(),
                },
                Entry {
                    name: "auth_token".into(),
                    content_type,
                    data: token.into_bytes(),
                },
            ];
            entries.extend(rest);
            entries
        })
}

/// Protocol data entries, half of them an authentication request's.
fn entries() -> impl Strategy<Value = Vec<Entry>> {
    prop_oneof![auth_entries(), prop::collection::vec(entry(), 0..5)]
}

/// The fields of any BTP packet; its type picks the ones it uses.
#[derive(Clone, Debug)]
struct BtpFields {
    packet_type: PacketType,
    request_id: u32,
    amount: u64,
    code: String,
    name: String,
    triggered_at: String,
    data: Vec<u8>,
    entries: Vec<Entry>,
}

fn btp_fields() -> impl Strategy<Value = BtpFields> {
    let packet_type = prop::sample::select(vec![
        PacketType::Response,
        PacketType::Error,
        PacketType::Message,
        PacketType::Transfer,
    ]);
    (
        packet_type,
        any::<u32>(),
        any::<u64>(),
        prop::collection::vec(0u8..0x80, 3).prop_map(|code| String::from_utf8(code).unwrap()),
        ia5(20),
        ia5(20),
        data(),
        entries(),
    )
        .prop_map(
            |(packet_type, request_id, amount, code, name, triggered_at, data, entries)| {
                BtpFields {
                    packet_type,
                    request_id,
                    amount,
                    code,
                    name,
                    triggered_at,
                    data,
                    entries,
                }
            },
        )
}

fn borrow(entries: &[Entry]) -> Vec<ProtocolDataEntry<'_>> {
    entries
        .iter()
        .map(|entry| ProtocolDataEntry {
            protocol_name: &entry.name,
            content_type: ContentType(entry.content_type),
            data: &entry.data,
        })
        .collect()
}

fn encode_protocol_data(entries: &[Entry]) -> Vec<u8> {
    ProtocolData::Entries(&borrow(entries)).to_vec()
}

fn encode_btp(fields: &BtpFields) -> Vec<u8> {
    let entries = borrow(&fields.entries);
    let protocol_data = ProtocolData::Entries(&entries);
    let body = match fields.packet_type {
        PacketType::Response => Body::Response(protocol_data),
        PacketType::Message => Body::Message(protocol_data),
        PacketType::Transfer => Body::Transfer {
            amount: fields.amount,
            protocol_data,
        },
        PacketType::Error => Body::Error(btp::Error {
            code: &fields.code,
            name: &fields.name,
            triggered_at: &fields.triggered_at,
            data: &fields.data,
            protocol_data,
        }),
    };
    btp::Packet {
        request_id: fields.request_id,
        body,
    }
    .to_vec()
}

/// Decodes a BTP packet from `input`, checks that it re-encodes identically
/// and that the lookups on its protocol data agree.
fn check_btp(input: &[u8]) -> Result<(), TestCaseError> {
    let Ok(packet) = btp::Packet::decode_exact(input) else {
        return Ok(());
    };
    prop_assert_eq!(packet.to_vec(), input);
    let protocol_data = packet.body.protocol_data();
    prop_assert_eq!(protocol_data.iter().count(), protocol_data.len());
    let first = protocol_data.first();
    if let Some(first) = first {
        prop_assert_eq!(protocol_data.find(first.protocol_name), Some(first));
    }
    prop_assert_eq!(first.is_none(), protocol_data.is_empty());
    if let Some(token) = btp::auth_token(&packet) {
        let entry = protocol_data.find("auth_token").unwrap();
        prop_assert_eq!(token.as_bytes(), entry.data);
        prop_assert_eq!(first.map(|first| first.protocol_name), Some("auth"));
    }
    Ok(())
}

/// Decodes `input` with every ILP entry point, checks that they agree and
/// that whatever decodes re-encodes identically.
fn check_ilp(input: &[u8]) -> Result<(), TestCaseError> {
    let packet = Packet::decode_exact(input).ok();
    if let Some(packet) = packet {
        prop_assert_eq!(packet.to_vec(), input);
    }
    // Prepare and Response accept exactly the packets of their types.
    let (prepare, response) = match packet {
        Some(Packet::Prepare(prepare)) => (Some(prepare), None),
        Some(Packet::Fulfill(fulfill)) => (None, Some(Response::Fulfill(fulfill))),
        Some(Packet::Reject(reject)) => (None, Some(Response::Reject(reject))),
        None => (None, None),
    };
    prop_assert_eq!(Prepare::decode_exact(input).ok(), prepare);
    prop_assert_eq!(Response::decode_exact(input).ok(), response);
    if let Some(Response::Reject(reject)) = response {
        check_amount_too_large(&reject)?;
    }
    Ok(())
}

/// [`AmountTooLarge::from_reject`] reads F08 rejects whose data decodes.
fn check_amount_too_large(reject: &Reject<'_>) -> Result<(), TestCaseError> {
    let read = AmountTooLarge::from_reject(reject);
    if reject.code == ErrorCode::F08_AMOUNT_TOO_LARGE {
        prop_assert_eq!(read, AmountTooLarge::decode_exact(reject.data).ok());
    } else {
        prop_assert_eq!(read, None);
    }
    if let Some(read) = read {
        prop_assert_eq!(&read.to_bytes()[..], reject.data);
    }
    Ok(())
}

proptest! {
    #[test]
    fn packets_never_panic_and_re_encode(input: Vec<u8>) {
        check_ilp(&input)?;
    }

    #[test]
    fn mutated_prepares_never_panic(
        amount: u64,
        unix_ms in 0..=Timestamp::MAX.unix_ms,
        condition: [u8; 32],
        destination in address(),
        data in data(),
        damage in damage(),
    ) {
        let encoded = Prepare {
            amount,
            expires_at: Timestamp::from_unix_ms(unix_ms),
            execution_condition: condition,
            destination: Address::parse(&destination).unwrap(),
            data: &data,
        }
        .to_vec();
        check_ilp(&apply(encoded, 1, &damage))?;
    }

    #[test]
    fn mutated_fulfills_never_panic(
        fulfillment: [u8; 32],
        data in data(),
        damage in damage(),
    ) {
        let encoded = Fulfill { fulfillment, data: &data }.to_vec();
        check_ilp(&apply(encoded, 1, &damage))?;
    }

    #[test]
    fn mutated_rejects_never_panic(
        code in error_code(),
        triggered_by in prop::option::of(address()),
        message in ".{0,20}",
        data in reject_data(),
        damage in damage(),
    ) {
        let encoded = Reject {
            code,
            triggered_by: triggered_by.as_deref().map(|address| Address::parse(address).unwrap()),
            message: &message,
            data: &data,
        }
        .to_vec();
        check_ilp(&apply(encoded, 1, &damage))?;
    }

    #[test]
    fn amount_too_large_never_panics(
        code in error_code(),
        input in prop::collection::vec(any::<u8>(), 0..24),
    ) {
        if let Ok(read) = AmountTooLarge::decode_exact(&input) {
            prop_assert_eq!(&read.to_bytes()[..], &input[..]);
        }
        let reject = Reject { code, triggered_by: None, message: "", data: &input };
        check_amount_too_large(&reject)?;
    }

    #[test]
    fn mutated_amount_too_large_never_panics(
        code in error_code(),
        data in reject_data(),
        mutations in mutations(),
    ) {
        let input = mutate(data, &mutations);
        if let Ok(read) = AmountTooLarge::decode_exact(&input) {
            prop_assert_eq!(&read.to_bytes()[..], &input[..]);
        }
        let reject = Reject { code, triggered_by: None, message: "", data: &input };
        check_amount_too_large(&reject)?;
    }

    #[test]
    fn btp_packets_never_panic_and_re_encode(input: Vec<u8>) {
        if let Ok(packet) = btp::Packet::decode_exact(&input) {
            prop_assert_eq!(packet.to_vec(), input);
        }
    }

    #[test]
    fn protocol_data_never_panics_and_re_encodes(input: Vec<u8>) {
        if let Ok(protocol_data) = ProtocolData::decode_exact(&input) {
            prop_assert_eq!(protocol_data.to_vec(), &input[..]);
            prop_assert_eq!(protocol_data.iter().count(), protocol_data.len());
        }
    }

    #[test]
    fn mutated_protocol_data_never_panics(entries in entries(), mutations in mutations()) {
        let encoded = mutate(encode_protocol_data(&entries), &mutations);
        if let Ok(protocol_data) = ProtocolData::decode_exact(&encoded) {
            prop_assert_eq!(protocol_data.to_vec(), &encoded[..]);
            prop_assert_eq!(protocol_data.iter().count(), protocol_data.len());
        }
    }

    #[test]
    fn mutated_btp_packets_never_panic(fields in btp_fields(), damage in damage()) {
        // Type octet and request ID come before the length prefix.
        check_btp(&apply(encode_btp(&fields), 5, &damage))?;
    }

    #[test]
    fn mutated_auth_requests_never_panic(
        request_id: u32,
        entries in auth_entries(),
        damage in damage(),
    ) {
        let entries = borrow(&entries);
        let encoded = btp::Packet {
            request_id,
            body: Body::Message(ProtocolData::Entries(&entries)),
        }
        .to_vec();
        check_btp(&apply(encoded, 5, &damage))?;
    }

    #[test]
    fn ildcp_responses_never_panic_and_re_encode(input: Vec<u8>) {
        if let Ok(response) = ildcp::Response::decode_exact(&input) {
            prop_assert_eq!(response.to_vec(), input);
        }
    }

    #[test]
    fn mutated_ildcp_responses_never_panic(
        address in address(),
        asset_scale: u8,
        asset_code in ia5(8),
        mutations in mutations(),
    ) {
        let encoded = ildcp::Response {
            address: Address::parse(&address).unwrap(),
            asset_scale,
            asset_code: &asset_code,
        }
        .to_vec();
        let encoded = mutate(encoded, &mutations);
        if let Ok(response) = ildcp::Response::decode_exact(&encoded) {
            prop_assert_eq!(response.to_vec(), encoded);
        }
    }

    #[test]
    fn prepares_round_trip(
        amount: u64,
        unix_ms in 0..=Timestamp::MAX.unix_ms,
        condition: [u8; 32],
        destination in address(),
        data in prop::collection::vec(any::<u8>(), 0..300),
    ) {
        let packet = Packet::Prepare(Prepare {
            amount,
            expires_at: Timestamp::from_unix_ms(unix_ms),
            execution_condition: condition,
            destination: Address::parse(&destination).unwrap(),
            data: &data,
        });
        let encoded = packet.to_vec();
        prop_assert_eq!(Packet::decode_exact(&encoded), Ok(packet));
    }

    #[test]
    fn timestamps_round_trip(unix_ms in 0..=Timestamp::MAX.unix_ms) {
        let time = Timestamp::from_unix_ms(unix_ms);
        prop_assert_eq!(Timestamp::parse(&time.to_ilp_bytes()), Ok(time));
    }

    #[test]
    fn timestamp_parsing_never_panics(text: [u8; 17]) {
        if let Ok(time) = Timestamp::parse(&text) {
            prop_assert_eq!(time.to_ilp_bytes(), text);
        }
    }

    #[test]
    fn address_parsing_never_panics(text: String) {
        if let Ok(address) = Address::parse(&text) {
            prop_assert_eq!(address.as_str(), text.as_str());
        }
    }

    #[test]
    fn generated_addresses_are_valid(text in address()) {
        prop_assert!(Address::parse(&text).is_ok());
    }
}
