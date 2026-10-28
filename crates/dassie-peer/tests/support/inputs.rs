//! Inputs for the never-panics properties (spec/decisions/tigerstyle.md): a
//! valid encoding of every kind of content, and ways to break an encoding
//! that, unlike random bytes, get past its first fields.

use dassie_oer::{Decode, Encode};
use dassie_peer::names::{Commitment, Name, Salt};
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

/// How many tags of content there are: 0 to 21, without gaps.
pub const KINDS: usize = 22;

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
        }),
        1 => Content::LinkStateUpdate(info),
        // A Fulfill, so that the node reads it as one: a Prepare travels
        // on a method.
        2 => Content::InterledgerPacket {
            request_id: 42,
            packet: &[13, 0x10, 0, 0, 0, 0, 0, 0, 0, 1],
        },
        3 => Content::LinkStateRequest(
            core::iter::once(info.node_id)
                .chain(info.info.neighbors())
                .collect(),
        ),
        4 => Content::Settlement {
            scheme: stub,
            amount: 1_000,
            data: &[0xab],
            sequence: 1,
        },
        5 => Content::NodeListHashRequest,
        6 => Content::NodeListRequest,
        7 => Content::Registration(info),
        8 => Content::PeeringInfoRequest { scheme: stub },
        9 => Content::PeeringTermination(TerminationReason::Owner),
        10 => Content::SettlementRequest { scheme: stub },
        11 => Content::NameCommit(Commitment([0xc1; 32])),
        // The longest name, so that a mutation can shorten it.
        12 => Content::NameUnseal {
            name: Name::parse("a-name-of-thirty-two-characters0").unwrap(),
            salt: Salt([0xc2; 32]),
        },
        13 => Content::NameRenew(Name::parse("alice").unwrap()),
        14 => Content::NameListHashRequest,
        15 => Content::NameListRequest,
        16 => Content::MethodPacket {
            request_id: 42,
            scheme: SchemeId::parse("hub-stub").unwrap(),
            packet: &[12, 0x10, 0, 0, 0, 0, 0, 0, 0, 1],
            commitment: &[0xc0; 96],
        },
        17 => Content::MethodAdd {
            scheme: stub,
            data: &[1, 2, 3],
        },
        18 => Content::MethodDrop { scheme: stub },
        19 => Content::Subscription { subscribe: true },
        // A snapshot is opaque to the peer protocol: bytes of its size.
        20 => Content::Snapshot(&[0x5a; 64]),
        // A Reject, with a release as long as a hub's: opaque to the peer
        // protocol.
        21 => Content::HubReject {
            request_id: 42,
            packet: &[14, 0x10, 0, 0, 0, 0, 0, 0, 0, 1],
            release: &[0x5e; 80],
        },
        _ => panic!("no content has tag {tag}"),
    };
    assert_eq!(content.kind().tag(), u64::try_from(tag).unwrap());
    content.to_vec()
}
