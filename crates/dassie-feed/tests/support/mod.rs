//! What the tests and the vector generator share: the fixed key and the
//! feeds they sign.

#![allow(dead_code, clippy::indexing_slicing, clippy::cast_possible_truncation)]

use dassie_feed::{Feed, MAX_METHOD_ID_BYTES, MAX_METHODS, MAX_PPM, Method, Rate};
use dassie_identity::NodeKey;

/// The seed of the key the vectors are signed with: the bytes 0 to 31.
pub const SEED: [u8; 32] = {
    let mut seed = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        seed[index] = index as u8;
        index += 1;
    }
    seed
};

/// The key the vectors are signed with.
pub fn key() -> NodeKey {
    NodeKey::from_bytes(&SEED)
}

/// Another key, for a node that is not the one subscribed to.
pub fn other_key() -> NodeKey {
    NodeKey::from_bytes(&[0xa5; 32])
}

/// The largest packet of the test feeds' methods, unless one says
/// otherwise.
pub const MAX_PACKET: u64 = 100_000_000;

/// A feed of `key`'s on these methods, with these rates, and fixed other
/// fields.
pub fn feed_of<'a>(key: &NodeKey, ids: Vec<&'a str>, rates: Vec<Rate>) -> Feed<'a> {
    let methods = ids
        .into_iter()
        .map(|id| Method {
            id,
            max_packet_amount: MAX_PACKET,
        })
        .collect();
    Feed {
        public_key: *key.public_key().as_bytes(),
        sequence_ms: 1_790_000_000_000,
        valid_until_ms: 1_790_000_090_000,
        fee_ppm: 1_000,
        methods,
        rates,
    }
}

/// A heartbeat of a node that converts nothing.
pub fn empty_feed() -> Feed<'static> {
    feed_of(&key(), vec![], vec![])
}

/// A node that converts between the testnet's DSUSD hub and its XRP hub,
/// both ways, with a spread.
pub fn two_method_feed() -> Feed<'static> {
    let mut feed = feed_of(
        &key(),
        vec!["hub-xrpl-testnet-dsusd", "hub-xrpl-testnet-xrp"],
        vec![
            Rate {
                from: 0,
                to: 1,
                rate_e15: 2_345_000_000_000_000,
            },
            Rate {
                from: 1,
                to: 0,
                rate_e15: 421_000_000_000_000,
            },
        ],
    );
    feed.methods[1].max_packet_amount = 25_000_000;
    feed
}

/// The method IDs of the largest feed: [`MAX_METHODS`] of the longest.
pub fn longest_ids() -> Vec<String> {
    (0..MAX_METHODS)
        .map(|index| {
            let mut id = "z".repeat(MAX_METHOD_ID_BYTES - 1);
            id.push(char::from(b'0' + index as u8));
            id
        })
        .collect()
}

/// The largest feed: every method, every pair, every field at its largest.
pub fn largest_feed(ids: &[String]) -> Feed<'_> {
    let methods: Vec<&str> = ids.iter().map(String::as_str).collect();
    let count = methods.len() as u8;
    let mut rates = Vec::new();
    for from in 0..count {
        for to in (0..count).filter(|to| *to != from) {
            rates.push(Rate {
                from,
                to,
                rate_e15: u64::MAX - u64::from(from) * 256 - u64::from(to),
            });
        }
    }
    let mut feed = feed_of(&key(), methods, rates);
    feed.sequence_ms = u64::MAX - 1;
    feed.valid_until_ms = u64::MAX;
    feed.fee_ppm = MAX_PPM;
    for (index, method) in feed.methods.iter_mut().enumerate() {
        method.max_packet_amount = u64::MAX - index as u64;
    }
    feed
}

/// Lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Bytes from hex.
pub fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap_or_default())
        .collect()
}
