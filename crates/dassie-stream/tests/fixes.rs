//! Cases where the original deviated from RFC 29 or was otherwise wrong
//! (spec 4 §11, ILP-11, ILP-13). Each test fails against the original.

mod support;

use dassie_oer::Writer;
use dassie_stream::allocate::{Claim, ExceedsCapacity, allocate};
use dassie_stream::crypto::{DecryptionFailure, Keys, OVERHEAD};
use dassie_stream::frame::Frame;
use dassie_stream::packet::{Frames, Packet, PacketType};
use dassie_stream::receipt::{self, ReceiptSecret};
use dassie_stream::token::{MAX_TOKEN_LENGTH, ServerSecret, TokenContents};
use support::hex;
use support::sim::TestRng;

fn encode(packet: &Packet<'_>) -> Vec<u8> {
    let mut buffer = vec![0; packet.encoded_len()];
    packet.encode_to(&mut Writer::new(&mut buffer));
    buffer
}

#[test]
fn ilp_11_unknown_frames_are_skipped() {
    // One known frame, one of type 0x7f, then another known frame. Each
    // frame is type, length, content; a var-uint content is itself length
    // and value.
    let mut packet = hex("010c0100010001");
    packet.push(3); // three frames
    packet.extend_from_slice(&hex("05020101")); // ConnectionMaxStreamId(1)
    packet.extend_from_slice(&hex("7f03aabbcc")); // unknown
    packet.extend_from_slice(&hex("03020102")); // ConnectionMaxData(2)
    let parsed = Packet::parse(&packet).unwrap();
    let frames: Vec<Frame<'_>> = parsed.frames.iter().collect();
    assert_eq!(
        frames,
        [
            Frame::ConnectionMaxStreamId { max_stream_id: 1 },
            Frame::ConnectionMaxData { max_offset: 2 },
        ]
    );
}

#[test]
fn ilp_11_bytes_after_the_frames_are_ignored() {
    let mut packet = encode(&Packet {
        sequence: 1,
        packet_type: PacketType::Prepare,
        prepare_amount: 0,
        frames: Frames::List(&[]),
    });
    packet.extend_from_slice(&[0; 40]);
    assert_eq!(Packet::parse(&packet).unwrap().sequence, 1);
}

#[test]
fn values_above_u64_saturate() {
    // StreamMaxMoney with a nine-octet receive max (2^64).
    let packet = hex("010c010001000101120f017b090100000000000000000201c8");
    let parsed = Packet::parse(&packet).unwrap();
    let frame = parsed.frames.iter().next().unwrap();
    assert_eq!(
        frame,
        Frame::StreamMaxMoney {
            stream_id: 123,
            receive_max: u64::MAX,
            total_received: 456
        }
    );
}

#[test]
fn an_empty_new_address_parses() {
    // RFC 29 allows a ConnectionNewAddress without an address; the
    // original's schema required at least one character.
    let mut packet = hex("010c0100010001");
    packet.push(1);
    packet.extend_from_slice(&hex("020100"));
    let parsed = Packet::parse(&packet).unwrap();
    assert_eq!(
        parsed.frames.iter().next(),
        Some(Frame::ConnectionNewAddress {
            source_account: None
        })
    );
}

#[test]
fn ilp_13_decryption_failure_is_a_value() {
    let keys = Keys::derive(b"secret");
    let mut output = [0u8; 64];
    assert_eq!(
        keys.cipher.decrypt(&[0; 40], &mut output),
        Err(DecryptionFailure)
    );
    assert_eq!(
        keys.cipher.decrypt(&[0; 10], &mut output),
        Err(DecryptionFailure)
    );
    let mut buffer = vec![0u8; OVERHEAD + 4];
    buffer[OVERHEAD..].copy_from_slice(b"data");
    let length = keys.cipher.encrypt_with_iv([1; 12], 4, &mut buffer);
    buffer[length - 1] ^= 1;
    assert_eq!(
        keys.cipher.decrypt(&buffer, &mut output),
        Err(DecryptionFailure)
    );
}

#[test]
fn allocation_matches_the_original() {
    let claims = |list: &[(u64, u64)]| -> Vec<Claim> {
        list.iter()
            .map(|&(shares, capacity)| Claim { shares, capacity })
            .collect()
    };
    let run = |total, list: &[(u64, u64)]| {
        let claims = claims(list);
        let mut output = vec![0; claims.len()];
        allocate(total, &claims, &mut output).map(|()| output)
    };
    assert_eq!(
        run(90, &[(10, 30), (10, 30), (10, 30)]),
        Ok(vec![30, 30, 30])
    );
    assert_eq!(
        run(510, &[(1000, 10), (10, 1000), (10, 1000)]),
        Ok(vec![10, 250, 250])
    );
    assert_eq!(
        run(100, &[(10, 1000), (10, 1000), (10, 1000)]),
        Ok(vec![34, 33, 33])
    );
    assert_eq!(
        run(3001, &[(10, 1000), (10, 1000), (10, 1000)]),
        Err(ExceedsCapacity)
    );
}

#[test]
fn server_tokens_are_stateless_and_authenticated() {
    // The original kept credentials in a map that never expired; tokens
    // carry their parameters encrypted under the server secret instead.
    let mut rng = TestRng::new(1);
    let secret = ServerSecret::new([5; 32]);
    let receipt = ReceiptSecret {
        nonce: [1; 16],
        secret: [2; 32],
    };
    let mut token = [0u8; MAX_TOKEN_LENGTH];
    let contents = TokenContents {
        connection_tag: b"invoice-42",
        receipt: Some(receipt),
    };
    let length = secret.issue_token(&mut rng, &contents, &mut token);
    let token = &token[..length];

    let restarted = ServerSecret::new([5; 32]);
    let mut scratch = [0u8; MAX_TOKEN_LENGTH];
    assert_eq!(restarted.open_token(token, &mut scratch), Ok(contents));
    assert_eq!(restarted.shared_secret(token), secret.shared_secret(token));

    let mut tampered = token.to_vec();
    tampered[20] ^= 1;
    assert!(restarted.open_token(&tampered, &mut scratch).is_err());
    assert!(
        ServerSecret::new([6; 32])
            .open_token(token, &mut scratch)
            .is_err()
    );
}

#[test]
fn receipts_match_rfc_39_and_the_fixture_layout() {
    let secret = ReceiptSecret {
        nonce: [0; 16],
        secret: [3; 32],
    };
    let created = receipt::create(&secret, 1, 500);
    // The fixture's receipt has the same layout: version, nonce, stream ID,
    // amount 500, then the HMAC.
    let fixture = hex(
        "01000000000000000000000000000000000100000000000001f4c122fa0252debb672d591823f710e99516d67385cf397da87cc8f9e8c8bf5130",
    );
    assert_eq!(created[..26], fixture[..26]);
    let verified = receipt::verify(&created, &secret.secret).unwrap();
    assert_eq!((verified.stream_id, verified.total_received), (1, 500));
    assert!(receipt::verify(&fixture, &secret.secret).is_err());
}
