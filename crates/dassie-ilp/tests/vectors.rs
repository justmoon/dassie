//! Golden vectors produced by the original's lib-protocol-ilp, -ildcp and
//! -utils (testdata/original).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use dassie_ilp::btp::{self, Body, ContentType, ProtocolData, ProtocolDataEntry};
use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode, Fulfill, Packet, Prepare, Reject, Timestamp, ildcp,
};
use dassie_oer::{Decode, Encode};
use serde_json::Value;
use support::{hex, str_field, to_hex, vectors};

fn decimal(value: &Value, key: &str) -> u64 {
    str_field(value, key).parse().unwrap()
}

fn array32(text: &str) -> [u8; 32] {
    hex(text).try_into().unwrap()
}

#[test]
fn ilp_packets() {
    let vectors = vectors("vectors.json");
    let packets = vectors["packets"].as_array().unwrap();
    assert_eq!(packets.len(), 3);
    for vector in packets {
        let encoded = hex(str_field(vector, "encoded"));
        assert_eq!(str_field(vector, "encoded"), str_field(vector, "reencoded"));
        let data = &vector["packet"]["data"];
        let payload = hex(str_field(data, "data"));
        let expected = match vector["packet"]["type"].as_u64().unwrap() {
            12 => Packet::Prepare(Prepare {
                amount: decimal(data, "amount"),
                expires_at: Timestamp::parse(
                    str_field(data, "expiresAt").as_bytes().try_into().unwrap(),
                )
                .unwrap(),
                execution_condition: array32(str_field(data, "executionCondition")),
                destination: Address::parse(str_field(data, "destination")).unwrap(),
                data: &payload,
            }),
            13 => Packet::Fulfill(Fulfill {
                fulfillment: array32(str_field(data, "fulfillment")),
                data: &payload,
            }),
            _ => Packet::Reject(Reject {
                code: ErrorCode::new(str_field(data, "code").as_bytes().try_into().unwrap())
                    .unwrap(),
                triggered_by: Some(Address::parse(str_field(data, "triggeredBy")).unwrap()),
                message: str_field(data, "message"),
                data: &payload,
            }),
        };
        let name = str_field(vector, "name");
        assert_eq!(
            Packet::decode_exact(&encoded),
            Ok(expected),
            "decoding {name}"
        );
        assert_eq!(
            to_hex(&expected.to_vec()),
            to_hex(&encoded),
            "encoding {name}"
        );
    }
}

#[test]
fn ildcp_response_and_constants() {
    let vectors = vectors("vectors.json");
    let vector = &vectors["ildcp"][0];
    let encoded = hex(str_field(vector, "encoded"));
    let response = &vector["response"];
    let expected = ildcp::Response {
        address: Address::parse(str_field(response, "address")).unwrap(),
        asset_scale: u8::try_from(response["assetScale"].as_u64().unwrap()).unwrap(),
        asset_code: str_field(response, "assetCode"),
    };
    assert_eq!(ildcp::Response::decode_exact(&encoded), Ok(expected));
    assert_eq!(expected.to_vec(), encoded);

    assert_eq!(ildcp::ADDRESS.as_str(), str_field(&vectors, "ildcpAddress"));
    assert_eq!(
        to_hex(&ildcp::CONDITION),
        str_field(&vectors, "ildcpCondition")
    );
    assert_eq!(
        dassie_ilp::condition_for(&ildcp::FULFILLMENT),
        ildcp::CONDITION
    );
}

#[test]
fn amount_too_large_data() {
    let vectors = vectors("vectors.json");
    for vector in vectors["f08"].as_array().unwrap() {
        let data = AmountTooLarge {
            received_amount: decimal(vector, "receivedAmount"),
            maximum_amount: decimal(vector, "maximumAmount"),
        };
        let encoded = hex(str_field(vector, "encoded"));
        assert_eq!(data.to_bytes().as_slice(), encoded.as_slice());
        assert_eq!(AmountTooLarge::decode_exact(&encoded), Ok(data));
    }
}

#[test]
fn timestamps() {
    let vectors = vectors("vectors.json");
    for vector in vectors["timestamps"].as_array().unwrap() {
        let time = Timestamp::from_unix_ms(vector["ms"].as_u64().unwrap());
        let text = str_field(vector, "text");
        assert_eq!(time.to_ilp_bytes().as_slice(), text.as_bytes());
        assert_eq!(
            Timestamp::parse(text.as_bytes().try_into().unwrap()),
            Ok(time)
        );
    }
}

/// The protocol data entries of a BTP vector, payloads decoded from hex.
fn entry_values(value: &Value) -> Vec<(String, u8, Vec<u8>)> {
    value["protocolData"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                str_field(entry, "protocolName").to_owned(),
                u8::try_from(entry["contentType"].as_u64().unwrap()).unwrap(),
                hex(str_field(entry, "data")),
            )
        })
        .collect()
}

fn entry_list(values: &[(String, u8, Vec<u8>)]) -> Vec<ProtocolDataEntry<'_>> {
    values
        .iter()
        .map(|(name, content_type, data)| ProtocolDataEntry {
            protocol_name: name,
            content_type: ContentType(*content_type),
            data,
        })
        .collect()
}

fn check_btp(vector: &Value, list: &[ProtocolDataEntry<'_>], body: Body<'_>) {
    let encoded = hex(str_field(vector, "encoded"));
    let packet = btp::Packet {
        request_id: u32::try_from(vector["value"]["requestId"].as_u64().unwrap()).unwrap(),
        body,
    };
    assert_eq!(to_hex(&packet.to_vec()), to_hex(&encoded));

    let decoded = btp::Packet::decode_exact(&encoded).unwrap();
    assert_eq!(decoded.request_id, packet.request_id);
    // Decoded protocol data is held in its encoding; compare entry by entry.
    let got: Vec<_> = decoded.body.protocol_data().iter().collect();
    assert_eq!(got, list);
}

#[test]
fn btp_packets() {
    let vectors = vectors("vectors.json");
    let btp = &vectors["btp"];

    for key in ["message", "response"] {
        let values = entry_values(&btp[key]["value"]);
        let list = entry_list(&values);
        let protocol_data = ProtocolData::Entries(&list);
        let body = if key == "message" {
            Body::Message(protocol_data)
        } else {
            Body::Response(protocol_data)
        };
        check_btp(&btp[key], &list, body);
    }

    let transfer = &btp["transfer"];
    let values = entry_values(&transfer["value"]);
    let list = entry_list(&values);
    let body = Body::Transfer {
        amount: decimal(&transfer["value"], "amount"),
        protocol_data: ProtocolData::Entries(&list),
    };
    check_btp(transfer, &list, body);

    let error = &btp["error"];
    let error_value = &error["value"];
    let error_data = hex(str_field(error_value, "data"));
    let values = entry_values(error_value);
    let list = entry_list(&values);
    let body = Body::Error(btp::Error {
        code: str_field(error_value, "code"),
        name: str_field(error_value, "name"),
        triggered_at: str_field(error_value, "triggeredAt"),
        data: &error_data,
        protocol_data: ProtocolData::Entries(&list),
    });
    check_btp(error, &list, body);

    let message = hex(str_field(&btp["message"], "encoded"));
    let packet = btp::Packet::decode_exact(&message).unwrap();
    assert_eq!(btp::auth_token(&packet), Some("secret-token"));
}
