//! The golden vectors of `tests/data/vectors.json`
//! (spec/areas/peer-protocol.md#test-vectors): the code makes exactly their
//! bytes, and reads every message as its own. Regenerate them only for a new
//! version of the protocol (spec/decisions/protocol-upgrades.md).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[allow(dead_code)]
#[path = "support/inputs.rs"]
mod inputs;
#[path = "support/vectors.rs"]
mod vectors;

use std::collections::BTreeMap;

use dassie_oer::{Decode, Encode};
use dassie_peer::wire::{Content, SignedNodeInfo, peek_kind};
use serde_json::Value;

fn file() -> Value {
    serde_json::from_str(include_str!("data/vectors.json")).unwrap()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn table(value: &Value) -> BTreeMap<String, Vec<u8>> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, hex)| (name.clone(), unhex(hex.as_str().unwrap())))
        .collect()
}

#[test]
fn the_keys_and_version_are_the_ones_the_vectors_name() {
    let file = file();
    assert_eq!(file["version"], vectors::VERSION);
    for (name, key) in [("nodeA", vectors::node_a()), ("nodeB", vectors::node_b())] {
        assert_eq!(
            unhex(file[name]["publicKey"].as_str().unwrap()),
            key.public_key().as_bytes()
        );
        assert_eq!(file[name]["nodeId"], key.node_id().as_str());
    }
}

#[test]
fn the_link_state_is_pinned_and_verifies() {
    let bytes = unhex(file()["linkState"].as_str().unwrap());
    assert_eq!(bytes, vectors::link_state());
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(signed.verify().unwrap(), vectors::node_a().public_key());
    assert_eq!(signed.node_id, vectors::node_a().node_id());
}

// Every message kind has a vector, and each reads as its kind and writes
// back to the same bytes.
#[test]
fn every_message_is_pinned() {
    let pinned = table(&file()["messages"]);
    assert_eq!(pinned, vectors::messages());
    assert_eq!(pinned.len(), inputs::KINDS - 1, "one per kind, but tag 5");
    for bytes in pinned.values() {
        let content = Content::decode_exact(bytes).unwrap();
        assert_eq!(content.to_vec(), *bytes);
        assert_eq!(peek_kind(bytes), Some(content.kind()));
    }
}

#[test]
fn every_answer_is_pinned() {
    assert_eq!(table(&file()["answers"]), vectors::answers());
}
