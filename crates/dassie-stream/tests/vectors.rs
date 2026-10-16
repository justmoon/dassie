//! Golden vectors produced by the original's lib-protocol-stream
//! (testdata/original): all 53 packet fixtures and the crypto vectors.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use dassie_ilp::Address;
use dassie_oer::Writer;
use dassie_stream::crypto::{Keys, OVERHEAD, sha256};
use dassie_stream::frame::{ErrorCode, Frame};
use dassie_stream::packet::{Frames, Packet, PacketType};
use serde_json::Value;
use support::{hex, str_field, to_hex, vectors};

fn uint(value: &Value, key: &str) -> u64 {
    // Values above u64 decode to u64::MAX (RFC 29 §5.2); the original kept
    // them exact (spec 4 §11.2).
    str_field(value, key).parse().unwrap_or(u64::MAX)
}

fn small(value: &Value, key: &str) -> u8 {
    u8::try_from(value[key].as_u64().unwrap()).unwrap()
}

fn expected_frame<'a>(frame: &'a Value, bytes: &'a mut Vec<u8>) -> Frame<'a> {
    let data = &frame["data"];
    match frame["type"].as_u64().unwrap() {
        0x01 => Frame::ConnectionClose {
            code: ErrorCode(small(data, "errorCode")),
            message: str_field(data, "errorMessage"),
        },
        0x02 => Frame::ConnectionNewAddress {
            source_account: Some(Address::parse(str_field(data, "sourceAccount")).unwrap()),
        },
        0x03 => Frame::ConnectionMaxData {
            max_offset: uint(data, "maxOffset"),
        },
        0x04 => Frame::ConnectionDataBlocked {
            max_offset: uint(data, "maxOffset"),
        },
        0x05 => Frame::ConnectionMaxStreamId {
            max_stream_id: uint(data, "maxStreamId"),
        },
        0x06 => Frame::ConnectionStreamIdBlocked {
            max_stream_id: uint(data, "maxStreamId"),
        },
        0x07 => Frame::ConnectionAssetDetails {
            asset_code: str_field(data, "sourceAssetCode"),
            asset_scale: small(data, "sourceAssetScale"),
        },
        0x10 => Frame::StreamClose {
            stream_id: uint(data, "streamId"),
            code: ErrorCode(small(data, "errorCode")),
            message: str_field(data, "errorMessage"),
        },
        0x11 => Frame::StreamMoney {
            stream_id: uint(data, "streamId"),
            shares: uint(data, "shares"),
        },
        0x12 => Frame::StreamMaxMoney {
            stream_id: uint(data, "streamId"),
            receive_max: uint(data, "receiveMax"),
            total_received: uint(data, "totalReceived"),
        },
        0x13 => Frame::StreamMoneyBlocked {
            stream_id: uint(data, "streamId"),
            send_max: uint(data, "sendMax"),
            total_sent: uint(data, "totalSent"),
        },
        0x14 => {
            *bytes = hex(str_field(data, "data"));
            Frame::StreamData {
                stream_id: uint(data, "streamId"),
                offset: uint(data, "offset"),
                data: bytes,
            }
        }
        0x15 => Frame::StreamMaxData {
            stream_id: uint(data, "streamId"),
            max_offset: uint(data, "maxOffset"),
        },
        0x16 => Frame::StreamDataBlocked {
            stream_id: uint(data, "streamId"),
            max_offset: uint(data, "maxOffset"),
        },
        0x17 => {
            *bytes = hex(str_field(data, "receipt"));
            Frame::StreamReceipt {
                stream_id: uint(data, "streamId"),
                receipt: bytes,
            }
        }
        other => panic!("unexpected frame type {other}"),
    }
}

fn encode(packet: &Packet<'_>) -> Vec<u8> {
    let mut buffer = vec![0; packet.encoded_len()];
    packet.encode_to(&mut Writer::new(&mut buffer));
    buffer
}

#[test]
fn all_53_packet_fixtures() {
    let vectors = vectors("vectors.json");
    let packets = vectors["packets"].as_array().unwrap();
    assert_eq!(packets.len(), 53, "every fixture of the original is tested");
    for vector in packets {
        let name = str_field(vector, "name");
        let encoded = hex(str_field(vector, "encoded"));
        let value = &vector["packet"];
        let mut storage: Vec<Vec<u8>> = value["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|_| Vec::new())
            .collect();
        let frames: Vec<Frame<'_>> = value["frames"]
            .as_array()
            .unwrap()
            .iter()
            .zip(storage.iter_mut())
            .map(|(frame, bytes)| expected_frame(frame, bytes))
            .collect();
        let packet_type = match value["packetType"].as_u64().unwrap() {
            12 => PacketType::Prepare,
            13 => PacketType::Fulfill,
            _ => PacketType::Reject,
        };
        let expected = Packet {
            sequence: uint(value, "sequence"),
            packet_type,
            prepare_amount: uint(value, "amount"),
            frames: Frames::List(&frames),
        };

        let decoded = Packet::parse(&encoded).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(decoded.sequence, expected.sequence, "{name}");
        assert_eq!(decoded.packet_type, expected.packet_type, "{name}");
        assert_eq!(decoded.prepare_amount, expected.prepare_amount, "{name}");
        let decoded_frames: Vec<Frame<'_>> = decoded.frames.iter().collect();
        assert_eq!(decoded_frames, frames, "{name}");

        if name.contains("greater_than_max_uint64") {
            // Saturated on decode, so it cannot re-encode to the original.
            continue;
        }
        assert_eq!(to_hex(&encode(&expected)), to_hex(&encoded), "{name}");
    }
}

#[test]
fn crypto_vectors() {
    let vectors = vectors("vectors.json");
    let crypto = &vectors["crypto"];
    let secret = hex(str_field(crypto, "secret"));
    let keys = Keys::derive(&secret);
    assert_eq!(
        to_hex(&Keys::encryption_key(&secret)),
        str_field(crypto, "encryptionKey")
    );
    assert_eq!(
        to_hex(keys.fulfillment_key()),
        str_field(crypto, "fulfillmentKey")
    );
    for vector in crypto["fulfillments"].as_array().unwrap() {
        let data = hex(str_field(vector, "data"));
        assert_eq!(
            to_hex(&keys.fulfillment(&data)),
            str_field(vector, "fulfillment")
        );
        assert_eq!(keys.condition(&data), sha256(&keys.fulfillment(&data)));
    }
    assert_eq!(
        to_hex(&sha256(&[0; 32])),
        str_field(crypto, "zeroCondition")
    );

    let encryptions = crypto["encryptions"].as_array().unwrap();
    assert_eq!(
        str_field(&encryptions[0], "ciphertext"),
        str_field(crypto, "originalCiphertext"),
        "the first vector is the original's own fixture"
    );
    for vector in encryptions {
        let plaintext = hex(str_field(vector, "plaintext"));
        let ciphertext = hex(str_field(vector, "ciphertext"));
        // The original drew its nonce from a seeded PRNG; it is the first
        // twelve bytes of the ciphertext.
        let iv: [u8; 12] = ciphertext[..12].try_into().unwrap();
        let mut buffer = vec![0; OVERHEAD + plaintext.len()];
        buffer[OVERHEAD..].copy_from_slice(&plaintext);
        let length = keys
            .cipher
            .encrypt_with_iv(iv, plaintext.len(), &mut buffer);
        assert_eq!(to_hex(&buffer[..length]), to_hex(&ciphertext));

        let mut output = vec![0; ciphertext.len()];
        assert_eq!(
            keys.cipher.decrypt(&ciphertext, &mut output),
            Ok(plaintext.as_slice())
        );
    }
}
