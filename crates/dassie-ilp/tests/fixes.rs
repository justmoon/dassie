//! Cases where the original was wrong or lenient (spec 4 §1, ILP-3, NET-33).
//! Each test fails against the original's behaviour.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_ilp::{
    Address, ErrorClass, ErrorCode, InvalidAddress, Packet, ParseErrorKind, Prepare, Reject,
    Response, Timestamp,
};
use dassie_oer::{Decode, DecodeErrorKind, Encode};

const CONDITION: [u8; 32] = [7; 32];

fn prepare(destination: &str) -> Prepare<'_> {
    Prepare {
        amount: 1,
        expires_at: Timestamp::from_unix_ms(1_514_000_000_000),
        execution_condition: CONDITION,
        destination: Address::parse(destination).unwrap(),
        data: b"",
    }
}

#[test]
fn ilp_3_reject_may_have_an_empty_triggered_by() {
    let reject = Reject {
        code: ErrorCode::F02_UNREACHABLE,
        triggered_by: None,
        message: "",
        data: b"",
    };
    let encoded = reject.to_vec();
    assert_eq!(encoded, [14, 6, b'F', b'0', b'2', 0, 0, 0]);
    assert_eq!(
        Response::decode_exact(&encoded),
        Ok(Response::Reject(reject))
    );
}

#[test]
fn ilp_3_malformed_packets_are_errors_not_panics() {
    let valid = Packet::Prepare(prepare("test.alice")).to_vec();
    for cut in 0..valid.len() {
        assert!(
            Packet::decode_exact(&valid[..cut]).is_err(),
            "truncated at {cut}"
        );
    }
    let mut unknown = valid.clone();
    unknown[0] = 15;
    assert_eq!(
        Packet::decode_exact(&unknown).unwrap_err().kind,
        ParseErrorKind::UnknownType(15)
    );
    let mut trailing = valid;
    trailing.push(0);
    assert_eq!(
        Packet::decode_exact(&trailing).unwrap_err().kind,
        ParseErrorKind::Encoding(DecodeErrorKind::TrailingBytes)
    );
}

#[test]
fn timestamps_must_be_digits_and_real_dates() {
    // The original turned non-digits into NaN instead of failing.
    for text in [
        b"2017122301214054x",
        b"20171323012140549",
        b"20170229012140549",
        b"20171223242140549",
        b"20171223016040549",
        b"19691231235959999",
        b"+0171223012140549",
    ] {
        assert!(
            Timestamp::parse(text).is_err(),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    assert!(Timestamp::parse(b"20160229000000000").is_ok());
    assert!(Timestamp::parse(b"99991231235959999").is_ok());
}

#[test]
fn invalid_expiry_is_reported_as_such() {
    let mut encoded = Packet::Prepare(prepare("test.alice")).to_vec();
    // Type, length, then eight amount octets: the expiry starts at 10.
    encoded[10] = b'x';
    let error = Packet::decode_exact(&encoded).unwrap_err();
    assert_eq!(error.kind, ParseErrorKind::InvalidTimestamp);
    assert_eq!(error.offset, 10);
}

#[test]
fn addresses_follow_rfc_15() {
    // The original accepted any 7-bit string as an address.
    for (text, reason) in [
        ("", InvalidAddress::Length),
        ("test", InvalidAddress::NoSegments),
        ("bogus.alice", InvalidAddress::Scheme),
        ("test.", InvalidAddress::Segment),
        ("test..alice", InvalidAddress::Segment),
        ("test.al ice", InvalidAddress::Segment),
        ("test.alice!", InvalidAddress::Segment),
    ] {
        assert_eq!(Address::parse(text), Err(reason), "{text:?}");
    }
    for text in [
        "g.us.bank",
        "test.alice.~stream_1-x",
        "peer.config",
        "private.a",
        "local.x",
    ] {
        assert!(Address::parse(text).is_ok(), "{text:?}");
    }
    let too_long = format!("test.{}", "a".repeat(1019));
    assert_eq!(Address::parse(&too_long), Err(InvalidAddress::Length));
}

#[test]
fn invalid_destinations_fail_to_parse() {
    let mut encoded = Packet::Prepare(prepare("test.alice")).to_vec();
    let position = encoded
        .windows(5)
        .position(|window| window == b"alice")
        .unwrap();
    encoded[position + 2] = b' ';
    assert!(matches!(
        Packet::decode_exact(&encoded).unwrap_err().kind,
        ParseErrorKind::InvalidAddress(InvalidAddress::Segment)
    ));
}

#[test]
fn net_33_prefixes_match_whole_segments() {
    let address = Address::parse("test.alice.bob").unwrap();
    assert!(address.has_prefix(Address::parse("test.alice").unwrap()));
    assert!(address.has_prefix(address));
    assert!(!address.has_prefix(Address::parse("test.ali").unwrap()));
    assert!(
        !Address::parse("test.alicebob")
            .unwrap()
            .has_prefix(Address::parse("test.alice").unwrap())
    );
    assert_eq!(
        address.strip_prefix(Address::parse("test.alice").unwrap()),
        Some("bob")
    );
    assert_eq!(address.strip_prefix(address), None);
}

#[test]
fn reject_messages_are_limited_in_octets() {
    // RFC 27 limits the message to 8191 octets; the original counted UTF-16
    // code units, so 8191 two-octet characters passed.
    let message = "é".repeat(4096);
    assert_eq!(message.len(), 8192);
    let mut encoded = vec![14, 0x82, 0, 0, b'F', b'9', b'9', 0, 0x82, 0x20, 0x00];
    encoded.extend_from_slice(message.as_bytes());
    encoded.push(0);
    let body_length = u16::try_from(encoded.len() - 4).unwrap().to_be_bytes();
    encoded[2..4].copy_from_slice(&body_length);
    assert_eq!(
        Packet::decode_exact(&encoded).unwrap_err().kind,
        ParseErrorKind::Encoding(DecodeErrorKind::SizeOutOfRange)
    );
}

#[test]
fn error_codes_have_rfc_names_and_classes() {
    // The original called F02 "Unavailable".
    assert_eq!(ErrorCode::F02_UNREACHABLE.name(), "Unreachable");
    assert_eq!(
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY.class(),
        ErrorClass::Temporary
    );
    assert_eq!(
        ErrorCode::R00_TRANSFER_TIMED_OUT.class(),
        ErrorClass::Relative
    );
    let unknown = ErrorCode::new(*b"X42").unwrap();
    assert_eq!(
        (unknown.class(), unknown.name()),
        (ErrorClass::Unknown, "Unknown Error")
    );
    assert!(ErrorCode::new([0x80, b'0', b'0']).is_none());
}

#[test]
fn fulfillments_check_their_condition() {
    let fulfillment = [0; 32];
    let fulfill = dassie_ilp::Fulfill {
        fulfillment,
        data: b"",
    };
    assert!(fulfill.satisfies(&dassie_ilp::ildcp::CONDITION));
    assert!(!fulfill.satisfies(&CONDITION));
}

#[test]
fn ildcp_requests_are_recognised() {
    let now = Timestamp::from_unix_ms(1_514_000_000_000);
    let request = dassie_ilp::ildcp::request(now);
    assert!(dassie_ilp::ildcp::is_request(&request));
    assert_eq!(request.expires_at.unix_ms, now.unix_ms + 30_000);
    assert!(!dassie_ilp::ildcp::is_request(&prepare("peer.config")));
}

#[test]
fn max_packet_length_fits_the_largest_prepare() {
    let destination = format!("test.{}", "a".repeat(1018));
    let data = vec![0; dassie_ilp::MAX_DATA_LENGTH];
    let packet = Prepare {
        data: &data,
        ..prepare(&destination)
    };
    assert_eq!(packet.encoded_len(), dassie_ilp::MAX_PACKET_LENGTH);
}
