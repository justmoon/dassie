//! Properties: parsing never panics, accepted input re-encodes identically,
//! and well-formed packets round-trip.

use dassie_ilp::{Address, Packet, Prepare, Timestamp, btp, ildcp};
use dassie_oer::{Decode, Encode};
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

proptest! {
    #[test]
    fn packets_never_panic_and_re_encode(input: Vec<u8>) {
        if let Ok(packet) = Packet::decode_exact(&input) {
            prop_assert_eq!(packet.to_vec(), input);
        }
    }

    #[test]
    fn mutated_packets_never_panic(position: usize, value: u8, data: Vec<u8>) {
        let destination = Address::parse("test.alice.bob").unwrap();
        let mut encoded = Packet::Prepare(Prepare {
            amount: 1000,
            expires_at: Timestamp::from_unix_ms(1_600_000_000_000),
            execution_condition: [3; 32],
            destination,
            data: &data,
        })
        .to_vec();
        let index = position % encoded.len();
        encoded[index] = value;
        if let Ok(packet) = Packet::decode_exact(&encoded) {
            prop_assert_eq!(packet.to_vec(), encoded);
        }
    }

    #[test]
    fn btp_packets_never_panic_and_re_encode(input: Vec<u8>) {
        if let Ok(packet) = btp::Packet::decode_exact(&input) {
            prop_assert_eq!(packet.to_vec(), input);
        }
    }

    #[test]
    fn ildcp_responses_never_panic_and_re_encode(input: Vec<u8>) {
        if let Ok(response) = ildcp::Response::decode_exact(&input) {
            prop_assert_eq!(response.to_vec(), input);
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
