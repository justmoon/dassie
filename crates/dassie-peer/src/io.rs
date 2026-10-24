//! What goes in and out of the peer node: requests for the driver to send,
//! their results, replies to inbound requests, and events.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::wire::{Kind, SchemeId, TerminationReason};

/// Our number for a request we send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// The driver's number for a request it received, so a reply that comes
/// later finds its way back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InboundId(pub u64);

/// A request for the driver to send over QUIC, with `body`: on the
/// connection to the node at `url`, or, when `url` is empty, on the one a
/// periphery node opened to us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Its number, for the result.
    pub id: RequestId,
    /// The node it goes to.
    pub to: NodeId,
    /// The node's URL; empty for a periphery node.
    pub url: String,
    /// What it is.
    pub kind: Kind,
    /// The envelope.
    pub body: Vec<u8>,
    /// How long it may take.
    pub timeout_ms: u64,
}

/// Why a request got no response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// It surely did not reach the node: refused before sending, or the
    /// connection failed.
    NotDelivered,
    /// It may have reached the node: a timeout after sending, or a reset.
    Unknown,
}

/// The HTTP response to one of our requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The status.
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// The answer to a peer's settlement request, still to be given the amount
/// on its way.
#[derive(Debug)]
pub struct SettlementAnswer(pub(crate) alloc::boxed::Box<crate::auth::Accepted>);

// Two answers are the same if they answer the same request.
impl PartialEq for SettlementAnswer {
    fn eq(&self, other: &Self) -> bool {
        self.0.sender == other.0.sender && self.0.response_mac(&[]) == other.0.response_mac(&[])
    }
}

impl Eq for SettlementAnswer {}

/// The acknowledgement of an ILP packet from a peer, still to be given what
/// follows the verdict on a Fulfill: on a peering over a hub, the payer's
/// claim (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
#[derive(Debug)]
pub struct PacketAnswer {
    pub(crate) accepted: alloc::boxed::Box<crate::auth::Accepted>,
    /// Whether the packet is a Fulfill, which the reply accepts.
    pub(crate) fulfill: bool,
    /// Whether the packet came on a peering whose scheme uses commitments.
    pub(crate) hub: bool,
}

impl PacketAnswer {
    /// Whether the reply may carry bytes after the verdict: the packet is a
    /// Fulfill on a peering over a hub.
    #[must_use]
    pub const fn takes_extra(&self) -> bool {
        self.fulfill && self.hub
    }
}

// Two answers are the same if they answer the same request alike.
impl PartialEq for PacketAnswer {
    fn eq(&self, other: &Self) -> bool {
        self.accepted.sender == other.accepted.sender
            && self.accepted.response_mac(&[]) == other.accepted.response_mac(&[])
            && self.fulfill == other.fulfill
            && self.hub == other.hub
    }
}

impl Eq for PacketAnswer {}

/// A reply to an inbound request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The HTTP status (spec/areas/peer-protocol.md#statuses).
    pub status: u16,
    /// For 200, the `PeerResponse`; otherwise a short text for people.
    pub body: Vec<u8>,
}

impl Reply {
    /// A refusal with `status` and a reason for people.
    #[must_use]
    pub fn refuse(status: u16, reason: &str) -> Self {
        Self {
            status,
            body: reason.as_bytes().to_vec(),
        }
    }

    /// Whether it is a success.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        self.status == 200
    }
}

/// What handling an inbound request gave.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound<'a> {
    /// The reply, now.
    Reply(Reply),
    /// The reply comes later, through [`Output::Reply`]: the node first
    /// checks the requester's URL.
    Deferred,
    /// An ILP packet from a peer, and the answer that acknowledges it.
    Packet {
        /// The peer.
        from: NodeId,
        /// The ILP request ID.
        request_id: u32,
        /// The ILP packet.
        packet: &'a [u8],
        /// The commitment that came with a Prepare on a peering over a
        /// hub, for its scheme; `None` on any other peering, and with a
        /// Fulfill or Reject.
        commitment: Option<&'a [u8]>,
        /// The acknowledgement, once the node knows what follows the
        /// verdict. A Fulfill's says it was accepted, whatever the node does
        /// with it: each side books its own Fulfills
        /// (spec/decisions/own-fulfills.md), and nodes of 0.1 still read the
        /// reply as a verdict.
        answer: PacketAnswer,
    },
    /// A settlement not applied before. The node books it, then calls
    /// [`PeerNode::settlement_applied`](crate::PeerNode::settlement_applied)
    /// and sends `reply`; if it cannot book it, it refuses with 400 and
    /// nothing changes.
    Settlement {
        /// The node that settled.
        from: NodeId,
        /// The scheme.
        scheme: SchemeId,
        /// The amount, in the scheme ledger's units.
        amount: u64,
        /// The scheme's data.
        data: &'a [u8],
        /// Its sequence.
        sequence: u64,
        /// The acknowledgement.
        reply: Reply,
    },
    /// A peer asks us to settle on its scheme: the node decides how much,
    /// if anything (spec/decisions/own-fulfills.md), and answers with what it
    /// then has on its way to the peer (spec/decisions/ending-a-peering.md).
    SettlementRequest {
        /// The peer.
        from: NodeId,
        /// The scheme.
        scheme: SchemeId,
        /// The answer, once the node knows the amount.
        answer: SettlementAnswer,
    },
    /// A scheme's message from a peer, for the scheme.
    SchemeMessage {
        /// The peer.
        from: NodeId,
        /// The scheme.
        scheme: SchemeId,
        /// The message.
        message: &'a [u8],
        /// The acknowledgement.
        reply: Reply,
    },
}

/// The body of the reply to a Fulfill. Nodes of 0.1 read it as a verdict,
/// which "accepted" tells them to keep their own books by
/// (spec/decisions/own-fulfills.md).
pub const VERDICT_ACCEPTED: u8 = 1;

/// What the node hands its driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// A request to send.
    Request(Request),
    /// A deferred reply.
    Reply {
        /// The request it answers.
        inbound: InboundId,
        /// The reply.
        reply: Reply,
    },
}

/// What happened, for the node and the logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A peering began.
    Peered {
        /// The peer.
        node: NodeId,
        /// Its scheme.
        scheme: SchemeId,
        /// Whether the peer asked.
        inbound: bool,
    },
    /// The routes changed.
    RoutesChanged,
    /// A node joined the table.
    NodeAdded(NodeId),
    /// A node left the table.
    NodeRemoved(NodeId),
    /// A node registered with us, or renewed.
    Registered(NodeId),
    /// We are registered with a bootstrap node.
    RegisteredWith(NodeId),
    /// A packet we sent to a peer did not get through. If `definite`, it
    /// surely did not reach the peer; otherwise it may have.
    PacketFailed {
        /// The peer.
        peer: NodeId,
        /// The ILP request ID.
        request_id: u32,
        /// Whether it surely did not arrive.
        definite: bool,
    },
    /// A peer answered a Fulfill of ours with bytes after the verdict: on
    /// a peering over a hub, the payer's claim, for the hub's scheme to
    /// read (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A reply
    /// of the verdict alone, or with more than
    /// [`MAX_CLAIM_BYTES`](crate::wire::MAX_CLAIM_BYTES) after it, gives
    /// none.
    FulfillReply {
        /// The peer.
        peer: NodeId,
        /// The ILP request ID of the Prepare the Fulfill answered.
        request_id: u32,
        /// The bytes after the verdict.
        extra: Vec<u8>,
    },
    /// A peering attempt failed.
    PeeringFailed {
        /// The node asked.
        node: NodeId,
        /// Why, for people.
        reason: &'static str,
    },
    /// A peering is being ended: no new packets to or from the peer.
    Terminating {
        /// The peer.
        peer: NodeId,
        /// Why.
        reason: TerminationReason,
        /// Whether we began it.
        by_us: bool,
    },
    /// We decided to end a peering, and tell no one until we collected
    /// what the peer owes (spec/decisions/ending-a-peering.md).
    Depeering {
        /// The peer.
        peer: NodeId,
        /// Why.
        reason: TerminationReason,
    },
    /// A peering ended. Its accounts and settlement channel stay, and
    /// nothing is settled for its end (spec/decisions/ending-a-peering.md).
    PeeringEnded {
        /// The peer.
        peer: NodeId,
    },
    /// The settlement channel with a node we no longer peer with closed,
    /// once idle: what stays of it is on disk, and its accounts stay
    /// (spec/decisions/closing-channels.md).
    ChannelClosed {
        /// The node.
        peer: NodeId,
    },
    /// A peer is listed as our neighbor again, or no longer.
    Advertised {
        /// The peer.
        peer: NodeId,
        /// Whether it is listed.
        on: bool,
        /// Whether it was withdrawn only because the room our books give
        /// it stayed short of a packet: it is heard from, and not banned.
        short: bool,
    },
    /// Whether our messages reach a peer changed: they stopped once a
    /// Prepare, Fulfill or Reject of ours surely did not arrive, and reach
    /// it again once it answers any request of ours. While they do not, a
    /// Fulfill of ours could not count in the peer's books, so the node
    /// takes no new Prepare from it (spec/decisions/unreachable-peers.md).
    Reachable {
        /// The peer.
        peer: NodeId,
        /// Whether our messages reach it now.
        reachable: bool,
    },
    /// A settlement message arrived.
    SettlementDelivered {
        /// The node.
        peer: NodeId,
        /// Its sequence.
        sequence: u64,
    },
    /// A peer answered our request to settle: with how much it has on its
    /// way to us, or `None` if its answer does not say, as a node from
    /// before spec/decisions/ending-a-peering.md answers.
    SettlementRequestAnswered {
        /// The node.
        peer: NodeId,
        /// What it has on its way, in the scheme ledger's units.
        settling: Option<u64>,
    },
    /// A request to settle was refused because the peer holds no peering
    /// with us on that scheme, as when it has not yet taken in our
    /// acceptance: it may be asked again at once.
    SettlementRequestRefused {
        /// The node.
        peer: NodeId,
    },
    /// A request to settle surely never reached the peer: it may be asked
    /// again at once.
    SettlementRequestNotDelivered {
        /// The node.
        peer: NodeId,
    },
    /// A settlement message was refused for being malformed or out of
    /// place, which is a bug: it is kept, and sent again later.
    SettlementRefused {
        /// The node.
        peer: NodeId,
        /// Its sequence.
        sequence: u64,
        /// The status.
        status: u16,
    },
}

/// What the node has counted, for the logs and the simulations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Requests sent.
    pub requests_sent: u64,
    /// Requests that failed or got an error.
    pub requests_failed: u64,
    /// Requests dropped because too many were in flight.
    pub requests_dropped: u64,
    /// Inbound requests refused, by any status.
    pub inbound_refused: u64,
    /// Link states accepted.
    pub link_states_accepted: u64,
    /// Link states dropped as old, unknown, invalid or too frequent.
    pub link_states_dropped: u64,
    /// Peerings begun.
    pub peerings: u64,
    /// ILP responses given up on: refused, out of time, or no room to
    /// retry.
    pub answers_dropped: u64,
    /// Peerings ended.
    pub terminations: u64,
    /// Settlement messages acknowledged.
    pub settlements_delivered: u64,
    /// Settlement messages applied from others.
    pub settlements_applied: u64,
    /// Settlement messages that arrived again and were not applied twice.
    pub settlements_duplicate: u64,
    /// Settlement channels closed.
    pub channels_closed: u64,
    /// Settlement requests sent (spec/decisions/own-fulfills.md).
    pub settlements_requested: u64,
}
