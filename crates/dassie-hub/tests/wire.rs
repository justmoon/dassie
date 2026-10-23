//! The consensus wire format (spec/areas/encoding.md): every message
//! round-trips, every byte string decodes one way or not at all, and the
//! decoder never panics on what a peer sends.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_hub::consensus::wire::{
    Certified, Delivery, Fetch, MAX_DELIVERED_BLOCKS, MAX_DELIVERED_CERTIFICATES, Message, Proposal,
};
use dassie_hub::consensus::{Block, Certificate, Vote, VoteKind};
use dassie_hub::{Digest, Signers, ValidatorIndex};
use dassie_identity::Signature;
use dassie_oer::Encode;
use proptest::prelude::*;

fn digest() -> impl Strategy<Value = Digest> {
    any::<[u8; 32]>().prop_map(Digest)
}

fn signature() -> impl Strategy<Value = Signature> {
    (any::<[u8; 32]>(), any::<[u8; 32]>()).prop_map(|(a, b)| {
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(&a);
        bytes[32..].copy_from_slice(&b);
        Signature::from_bytes(bytes)
    })
}

fn kind() -> impl Strategy<Value = VoteKind> {
    prop_oneof![
        Just(VoteKind::Notarize),
        Just(VoteKind::Finalize),
        Just(VoteKind::Nullify)
    ]
}

fn block() -> impl Strategy<Value = Block> {
    (
        any::<u64>(),
        any::<u64>(),
        any::<u64>(),
        digest(),
        any::<u64>(),
        any::<u64>(),
        proptest::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(
            |(epoch, view, height, parent, parent_view, timestamp_ms, payload)| Block {
                epoch,
                view,
                height,
                parent,
                parent_view,
                timestamp_ms,
                payload,
            },
        )
}

fn vote() -> impl Strategy<Value = Vote> {
    (
        kind(),
        any::<(u64, u64)>(),
        digest(),
        0..16usize,
        signature(),
    )
        .prop_map(|(kind, (epoch, view), block, signer, signature)| Vote {
            kind,
            epoch,
            view,
            block: if kind == VoteKind::Nullify {
                Digest::default()
            } else {
                block
            },
            signer: ValidatorIndex::new(signer).unwrap(),
            signature,
        })
}

fn certificate() -> impl Strategy<Value = Certificate> {
    (
        kind(),
        any::<(u64, u64)>(),
        digest(),
        any::<u16>(),
        proptest::collection::vec(signature(), 16),
    )
        .prop_map(|(kind, (epoch, view), block, bits, signatures)| {
            let signers = Signers::from_bits(bits);
            Certificate {
                kind,
                epoch,
                view,
                block: if kind == VoteKind::Nullify {
                    Digest::default()
                } else {
                    block
                },
                signers,
                signatures: signatures.into_iter().take(signers.count()).collect(),
            }
        })
}

fn delivery() -> impl Strategy<Value = Delivery> {
    (
        proptest::collection::vec(block(), 0..4),
        proptest::option::of(certificate()),
        proptest::collection::vec(
            (certificate(), proptest::option::of(block()))
                .prop_map(|(certificate, block)| Certified { certificate, block }),
            0..4,
        ),
    )
        .prop_map(|(finalized, finalization, certified)| Delivery {
            finalized,
            finalization,
            certified,
        })
}

fn message() -> impl Strategy<Value = Message> {
    prop_oneof![
        (block(), signature())
            .prop_map(|(block, signature)| Message::Proposal(Proposal { block, signature })),
        vote().prop_map(Message::Vote),
        certificate().prop_map(Message::Certificate),
        (any::<u64>(), any::<u64>(), any::<u64>()).prop_map(
            |(height, epoch, view)| Message::Fetch(Fetch {
                height,
                epoch,
                view
            })
        ),
        delivery().prop_map(Message::Delivery),
    ]
}

proptest! {
    #[test]
    fn every_message_round_trips(message in message()) {
        let bytes = message.to_vec();
        prop_assert_eq!(bytes.len(), message.encoded_len());
        prop_assert_eq!(Message::decode_message(&bytes).unwrap(), message);
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Message::decode_message(&bytes);
    }

    #[test]
    fn what_decodes_re_encodes_to_the_same_bytes(
        message in message(),
        flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..4),
        cut in any::<prop::sample::Index>(),
    ) {
        let mut bytes = message.to_vec();
        for (at, byte) in flips {
            let at = at.index(bytes.len());
            bytes[at] = byte;
        }
        bytes.truncate(cut.index(bytes.len() + 1).max(1));
        if let Ok(decoded) = Message::decode_message(&bytes) {
            prop_assert_eq!(decoded.to_vec(), bytes);
        }
    }

    #[test]
    fn a_block_digest_is_over_its_encoding(block in block(), hub in digest()) {
        let digest = block.digest(&hub);
        let mut other = block.clone();
        other.payload.push(0);
        prop_assert_ne!(other.digest(&hub), digest);
        prop_assert_ne!(block.digest(&Digest([0xee; 32])), digest);
    }
}

#[test]
fn a_vote_is_laid_out_as_specified() {
    let vote = Vote {
        kind: VoteKind::Finalize,
        epoch: 0x0304,
        view: 0x0102,
        block: Digest([0xab; 32]),
        signer: ValidatorIndex::new(3).unwrap(),
        signature: Signature::from_bytes([0xcd; 64]),
    };
    let bytes = Message::Vote(vote).to_vec();
    // [1] tag, kind, epoch and view as 8 octets each, block, signer,
    // signature.
    let mut expected = vec![0x81, 0x01, 0, 0, 0, 0, 0, 0, 0x03, 0x04];
    expected.extend([0, 0, 0, 0, 0, 0, 0x01, 0x02]);
    expected.extend([0xab; 32]);
    expected.push(3);
    expected.extend([0xcd; 64]);
    assert_eq!(bytes, expected);
}

#[test]
fn malformed_values_are_refused() {
    let nullify = Vote {
        kind: VoteKind::Nullify,
        epoch: 0,
        view: 1,
        block: Digest::default(),
        signer: ValidatorIndex::new(0).unwrap(),
        signature: Signature::from_bytes([0; 64]),
    };
    let mut bytes = Message::Vote(nullify).to_vec();
    // A nullify naming a block.
    bytes[18] = 1;
    assert!(Message::decode_message(&bytes).is_err());
    // An unknown kind.
    let mut bytes = Message::Vote(nullify).to_vec();
    bytes[1] = 3;
    assert!(Message::decode_message(&bytes).is_err());
    // A signer beyond the largest set.
    let mut bytes = Message::Vote(nullify).to_vec();
    bytes[50] = 16;
    assert!(Message::decode_message(&bytes).is_err());
    // An unknown message.
    assert!(Message::decode_message(&[0x85]).is_err());
}

#[test]
fn a_certificate_has_one_signature_per_signer() {
    let certificate = Certificate {
        kind: VoteKind::Notarize,
        epoch: 0,
        view: 9,
        block: Digest([1; 32]),
        signers: Signers::from_bits(0b111),
        signatures: vec![Signature::from_bytes([2; 64]); 3],
    };
    let mut bytes = Message::Certificate(certificate).to_vec();
    // Two signers named, three signatures.
    bytes[51] = 0b011;
    assert!(Message::decode_message(&bytes).is_err());
}

#[test]
fn deliveries_are_bounded() {
    let block = Block {
        epoch: 0,
        view: 1,
        height: 1,
        parent: Digest::default(),
        parent_view: 0,
        timestamp_ms: 0,
        payload: Vec::new(),
    };
    let fits = Delivery {
        finalized: vec![block.clone(); MAX_DELIVERED_BLOCKS],
        finalization: None,
        certified: Vec::new(),
    };
    let bytes = Message::Delivery(fits).to_vec();
    assert!(Message::decode_message(&bytes).is_ok());
    // One more block than allowed: its count is refused before any block
    // is read.
    let mut bytes = bytes;
    bytes[1] = u8::try_from(MAX_DELIVERED_BLOCKS + 1).unwrap();
    assert!(Message::decode_message(&bytes).is_err());
    let _ = MAX_DELIVERED_CERTIFICATES;
}
