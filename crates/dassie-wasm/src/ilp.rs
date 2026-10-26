//! JavaScript bindings (wasm-bindgen) for ILP packets, for the STREAM
//! driver, test harnesses and the website. Packets cross the boundary as
//! `Uint8Array`s; a decoded packet is a read-only view object.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use dassie_oer::{Decode, Encode};
use wasm_bindgen::prelude::*;

use dassie_ilp::{Address, AmountTooLarge, ErrorCode, Fulfill, Packet, Prepare, Reject, Timestamp};

fn address(text: &str) -> Result<Address<'_>, JsError> {
    Address::parse(text).map_err(|error| JsError::new(&error.to_string()))
}

fn array32(bytes: &[u8], what: &str) -> Result<[u8; 32], JsError> {
    bytes
        .try_into()
        .map_err(|_| JsError::new(&alloc::format!("{what} must be 32 bytes")))
}

/// A decoded ILP packet. Fields that a packet type lacks are empty.
#[wasm_bindgen]
pub struct IlpPacket {
    packet_type: u8,
    amount: u64,
    expires_at_ms: f64,
    condition_or_fulfillment: Vec<u8>,
    address: String,
    code: String,
    message: String,
    data: Vec<u8>,
}

#[wasm_bindgen]
impl IlpPacket {
    /// 12 Prepare, 13 Fulfill, 14 Reject.
    #[wasm_bindgen(getter, js_name = type)]
    #[must_use]
    pub fn packet_type(&self) -> u8 {
        self.packet_type
    }

    /// A Prepare's amount.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn amount(&self) -> u64 {
        self.amount
    }

    /// A Prepare's expiry in milliseconds since the epoch.
    #[wasm_bindgen(getter, js_name = expiresAt)]
    #[must_use]
    pub fn expires_at(&self) -> f64 {
        self.expires_at_ms
    }

    /// A Prepare's condition or a Fulfill's fulfillment.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn hash(&self) -> Vec<u8> {
        self.condition_or_fulfillment.clone()
    }

    /// A Prepare's destination or a Reject's triggered-by address.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn address(&self) -> String {
        self.address.clone()
    }

    /// A Reject's code.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn code(&self) -> String {
        self.code.clone()
    }

    /// A Reject's message.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// The data field.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn data(&self) -> Vec<u8> {
        self.data.clone()
    }
}

/// Parses an ILP packet.
#[wasm_bindgen(js_name = decodeIlpPacket)]
pub fn decode_ilp_packet(bytes: &[u8]) -> Result<IlpPacket, JsError> {
    let packet = Packet::decode_exact(bytes).map_err(|error| JsError::new(&error.to_string()))?;
    let mut view = IlpPacket {
        packet_type: 0,
        amount: 0,
        expires_at_ms: 0.0,
        condition_or_fulfillment: Vec::new(),
        address: String::new(),
        code: String::new(),
        message: String::new(),
        data: Vec::new(),
    };
    match packet {
        Packet::Prepare(prepare) => {
            view.packet_type = dassie_ilp::TYPE_PREPARE;
            view.amount = prepare.amount;
            // Interledger timestamps end in year 9999, well within the
            // integers an f64 represents exactly.
            #[allow(clippy::cast_precision_loss)]
            {
                view.expires_at_ms = prepare.expires_at.unix_ms as f64;
            }
            view.condition_or_fulfillment = prepare.execution_condition.to_vec();
            view.address = String::from(prepare.destination.as_str());
            view.data = prepare.data.to_vec();
        }
        Packet::Fulfill(fulfill) => {
            view.packet_type = dassie_ilp::TYPE_FULFILL;
            view.condition_or_fulfillment = fulfill.fulfillment.to_vec();
            view.data = fulfill.data.to_vec();
        }
        Packet::Reject(reject) => {
            view.packet_type = dassie_ilp::TYPE_REJECT;
            view.address = String::from(reject.triggered_by.map_or("", |a| a.as_str()));
            view.code = String::from(reject.code.as_str());
            view.message = String::from(reject.message);
            view.data = reject.data.to_vec();
        }
    }
    Ok(view)
}

/// Encodes an ILP Prepare. `expiresAt` is in milliseconds since the epoch.
#[wasm_bindgen(js_name = encodePrepare)]
pub fn encode_prepare(
    amount: u64,
    expires_at: f64,
    condition: &[u8],
    destination: &str,
    data: &[u8],
) -> Result<Vec<u8>, JsError> {
    if !(0.0..=253_402_300_799_999.0).contains(&expires_at) {
        return Err(JsError::new("expiry out of range"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // Checked to be a non-negative integer-valued range above.
    let unix_ms = expires_at as u64;
    let prepare = Prepare {
        amount,
        expires_at: Timestamp::from_unix_ms(unix_ms),
        execution_condition: array32(condition, "condition")?,
        destination: address(destination)?,
        data,
    };
    Ok(prepare.to_vec())
}

/// Encodes an ILP Fulfill.
#[wasm_bindgen(js_name = encodeFulfill)]
pub fn encode_fulfill(fulfillment: &[u8], data: &[u8]) -> Result<Vec<u8>, JsError> {
    Ok(Fulfill {
        fulfillment: array32(fulfillment, "fulfillment")?,
        data,
    }
    .to_vec())
}

/// Encodes an ILP Reject. An empty `triggeredBy` is allowed.
#[wasm_bindgen(js_name = encodeReject)]
pub fn encode_reject(
    code: &str,
    triggered_by: &str,
    message: &str,
    data: &[u8],
) -> Result<Vec<u8>, JsError> {
    let code = <[u8; 3]>::try_from(code.as_bytes())
        .ok()
        .and_then(ErrorCode::new)
        .ok_or_else(|| JsError::new("an error code has three ASCII characters"))?;
    let triggered_by = if triggered_by.is_empty() {
        None
    } else {
        Some(address(triggered_by)?)
    };
    Ok(Reject {
        code,
        triggered_by,
        message,
        data,
    }
    .to_vec())
}

/// Encodes the data of an F08 Amount Too Large reject (RFC 27).
#[wasm_bindgen(js_name = encodeAmountTooLarge)]
#[must_use]
pub fn encode_amount_too_large(received_amount: u64, maximum_amount: u64) -> Vec<u8> {
    AmountTooLarge {
        received_amount,
        maximum_amount,
    }
    .to_bytes()
    .to_vec()
}

/// Reads the data of an F08 reject as `[receivedAmount, maximumAmount]`, or
/// nothing if it is malformed: the original sent F08 without data (ILP-4).
#[wasm_bindgen(js_name = decodeAmountTooLarge)]
#[must_use]
pub fn decode_amount_too_large(data: &[u8]) -> Option<Vec<u64>> {
    let parsed = AmountTooLarge::decode_exact(data).ok()?;
    Some(alloc::vec![parsed.received_amount, parsed.maximum_amount])
}
