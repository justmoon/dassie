//! Inputs for the never-panics properties (D16): a valid encoding of every
//! kind of content, and ways to break an encoding that, unlike random
//! bytes, get past its first fields.

use dassie_oer::{Decode, Encode};
use dassie_peer::wire::{Content, PeeringRequest, SchemeId, SignedNodeInfo, TerminationReason};
use proptest::prelude::*;
use proptest::sample::Index;

/// Where in an encoding a mutation applies.
#[derive(Clone, Copy, Debug)]
pub enum At {
    /// Anywhere.
    Anywhere(Index),
    /// This many bytes before the last: where the last fields are, which
    /// a mutation anywhere in a long encoding rarely hits.
    FromEnd(u8),
}

impl At {
    fn index(self, length: usize) -> usize {
        match self {
            Self::Anywhere(index) => index.index(length),
            Self::FromEnd(back) => length.saturating_sub(1 + usize::from(back)),
        }
    }
}

fn at() -> impl Strategy<Value = At> {
    prop_oneof![
        any::<Index>().prop_map(At::Anywhere),
        (0..16u8).prop_map(At::FromEnd),
    ]
}

/// One way to break an encoding.
#[derive(Clone, Debug)]
pub enum Mutation {
    /// Sets a byte.
    Set(At, u8),
    /// Adds one to a byte, or takes one away: a length or a count off by
    /// one.
    Nudge(At, bool),
    /// Cuts it short before a byte.
    Cut(At),
    /// Adds bytes at the end.
    Extend(Vec<u8>),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        3 => (at(), any::<u8>()).prop_map(|(at, byte)| Mutation::Set(at, byte)),
        2 => (at(), any::<bool>()).prop_map(|(at, up)| Mutation::Nudge(at, up)),
        2 => at().prop_map(Mutation::Cut),
        1 => prop::collection::vec(any::<u8>(), 1..64).prop_map(Mutation::Extend),
    ]
}

/// One to four mutations, applied in turn: half the time one alone, which
/// more often leaves the rest of the encoding valid.
pub fn mutations() -> impl Strategy<Value = Vec<Mutation>> {
    prop_oneof![
        prop::collection::vec(mutation(), 1),
        prop::collection::vec(mutation(), 2..=4),
    ]
}

/// `bytes` with `mutations` applied in turn.
pub fn mutate(bytes: &[u8], mutations: &[Mutation]) -> Vec<u8> {
    let mut bytes = bytes.to_vec();
    for mutation in mutations {
        let length = bytes.len();
        match mutation {
            Mutation::Set(at, byte) if length > 0 => bytes[at.index(length)] = *byte,
            Mutation::Nudge(at, up) if length > 0 => {
                let byte = &mut bytes[at.index(length)];
                *byte = if *up {
                    byte.wrapping_add(1)
                } else {
                    byte.wrapping_sub(1)
                };
            }
            Mutation::Cut(at) if length > 0 => bytes.truncate(at.index(length)),
            Mutation::Extend(tail) => bytes.extend(tail),
            _ => {}
        }
    }
    bytes
}

/// How many kinds of content there are: tags 0 to 10.
pub const KINDS: usize = 11;

/// A valid encoding of the content with CHOICE tag `tag`, carrying the
/// link state `info` where it has one.
pub fn content(tag: usize, info: &[u8]) -> Vec<u8> {
    let stub = SchemeId::parse("stub").unwrap();
    let info = SignedNodeInfo::decode_exact(info).unwrap();
    let content = match tag {
        0 => Content::PeeringRequest(PeeringRequest {
            scheme: stub,
            node_info: info,
            data: &[1, 2, 3],
            versions: (1..=8).collect(),
            credit: 1_100_000_000,
        }),
        1 => Content::LinkStateUpdate(info),
        // A Prepare, so that the node reads it as one.
        2 => Content::InterledgerPacket {
            request_id: 42,
            packet: &[12, 0x10, 0, 0, 0, 0, 0, 0, 0, 1],
        },
        3 => Content::LinkStateRequest(
            core::iter::once(info.info.node_id)
                .chain(info.info.neighbors())
                .collect(),
        ),
        4 => Content::Settlement {
            scheme: stub,
            amount: 1_000,
            data: &[0xab],
            sequence: 1,
        },
        5 => Content::SettlementMessage {
            scheme: stub,
            message: &[1, 2, 3],
        },
        6 => Content::NodeListHashRequest,
        7 => Content::NodeListRequest,
        8 => Content::Registration(info),
        9 => Content::PeeringInfoRequest { scheme: stub },
        10 => Content::PeeringTermination(TerminationReason::Owner),
        _ => panic!("no content has tag {tag}"),
    };
    assert_eq!(content.kind().tag(), u64::try_from(tag).unwrap());
    content.to_vec()
}
