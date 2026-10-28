//! What goes in and out of the peer node: requests for the driver to send,
//! their results, replies to inbound requests, and events.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::names::Name;
use crate::wire::{Endpoint, HubDetails, Kind, SchemeId, TerminationReason};

/// Our number for a request we send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// The driver's number for a request it received, so a reply that comes
/// later finds its way back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InboundId(pub u64);

/// A request for the driver to send over QUIC, with `body`: on the
/// connection to the node at `endpoint`, or, without one, on the one a
/// periphery node opened to us.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Its number, for the result.
    pub id: RequestId,
    /// The node it goes to.
    pub to: NodeId,
    /// Where the node takes QUIC; `None` for a periphery node, which is
    /// reached only over the connection it opened.
    pub endpoint: Option<Endpoint>,
    /// What it is.
    pub kind: Kind,
    /// The content.
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
    /// It may have reached the node: a timeout after sending, a reset, or
    /// an answer too short to hold a status.
    Unknown,
}

/// The response to one of our requests: a status
/// (spec/areas/peer-protocol.md#statuses) and a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// The status.
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// Whether a request answered with `status` was surely not acted upon, so
/// that its sender may take it as undelivered
/// (spec/areas/peer-protocol.md#statuses). Each of these says the request
/// changed nothing at the node that answered:
///
/// - The peer node's refusals: 400 and 409, and 429 and 503 for a rate or
///   a table that is full.
/// - The transport's, before the peer node sees the body
///   (`crates/dassie-node/src/peer_server.rs`): 429 while the sender's
///   failed requests are over their rate, 503 with no place among the
///   requests waiting, 413 for a body too large, 408 for one too slow and
///   400 for one broken.
/// - 503 too when the engine stopped before what the request changed was
///   on disk: its restart takes it back
///   (spec/areas/node.md#the-engine).
/// - 404 from a front door, which handles no peer request.
///
/// Every other status, and 200 above all, may mean it was acted upon.
#[must_use]
pub const fn not_acted_upon(status: u16) -> bool {
    matches!(status, 400 | 404 | 408 | 409 | 413 | 429 | 503)
}

/// The answer to a peer's settlement request, still to be given the amount
/// on its way: to the peer named.
#[derive(Debug, PartialEq, Eq)]
pub struct SettlementAnswer(pub(crate) NodeId);

/// The acknowledgement of an ILP packet from a peer, still to be given what
/// it carries on a Fulfill: on a peering with a method over a hub, the
/// payer's claim (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
#[derive(Debug, PartialEq, Eq)]
pub struct PacketAnswer {
    /// The peer it answers.
    pub(crate) peer: NodeId,
    /// Whether the packet is a Fulfill.
    pub(crate) fulfill: bool,
    /// Whether the packet came on a method that uses commitments, or, for a
    /// Fulfill or Reject, which travel on none, on a peering with such a
    /// method.
    pub(crate) hub: bool,
}

impl PacketAnswer {
    /// Whether the reply may carry a claim: the packet is a Fulfill on a
    /// peering with a method over a hub.
    #[must_use]
    pub const fn takes_claim(&self) -> bool {
        self.fulfill && self.hub
    }
}

/// A reply to an inbound request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The status (spec/areas/peer-protocol.md#statuses).
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
        /// The method a Prepare came on, by its scheme
        /// (spec/decisions/settlement-methods.md); `None` with a Fulfill or
        /// Reject, which answers its Prepare by request ID.
        scheme: Option<SchemeId>,
        /// The commitment that came with a Prepare on a method over a hub,
        /// for its scheme; `None` for one that came without
        /// (spec/designs/settlement-hubs.md#5-joining), on any other
        /// method, and with a Fulfill or Reject.
        commitment: Option<&'a [u8]>,
        /// The release that came with a Reject in a `hubReject`, for the
        /// scheme of the method its Prepare went on
        /// (spec/decisions/signed-rejects.md); `None` otherwise.
        release: Option<&'a [u8]>,
        /// The acknowledgement, once the node knows the claim it carries on
        /// a Fulfill over a hub. It says nothing of what the node does with
        /// the packet: each side books its own Fulfills
        /// (spec/decisions/own-fulfills.md).
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
}

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
    /// A further settlement method was added to a peering
    /// (spec/decisions/settlement-methods.md).
    MethodAdded {
        /// The peer.
        peer: NodeId,
        /// Its scheme.
        scheme: SchemeId,
        /// Whether the peer asked.
        inbound: bool,
    },
    /// We decided to drop a settlement method, and collect on it first,
    /// telling no one until we have (spec/decisions/settlement-methods.md):
    /// it takes no new packets from the peer.
    MethodWinding {
        /// The peer.
        peer: NodeId,
        /// Its scheme.
        scheme: SchemeId,
    },
    /// A settlement method was dropped from a peering that keeps another:
    /// nothing more is settled or sent on it, and what its accounts show
    /// stays owed (spec/decisions/settlement-methods.md).
    MethodDropped {
        /// The peer.
        peer: NodeId,
        /// Its scheme.
        scheme: SchemeId,
        /// Whether we dropped it; otherwise the peer told us it did.
        by_us: bool,
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
    /// surely did not reach the peer, or the peer did not act on it;
    /// otherwise it may have.
    PacketFailed {
        /// The peer.
        peer: NodeId,
        /// The ILP request ID.
        request_id: u32,
        /// Whether it surely did not arrive, or was not acted upon.
        definite: bool,
        /// The status the peer answered with, if it answered one.
        status: Option<u16>,
    },
    /// A peer answered a Fulfill of ours with a claim: on a peering with a
    /// method over a hub, the payer's, for the hub's scheme to read
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). An empty
    /// reply, or one longer than
    /// [`MAX_CLAIM_BYTES`](crate::wire::MAX_CLAIM_BYTES), gives none.
    FulfillReply {
        /// The peer.
        peer: NodeId,
        /// The ILP request ID of the Prepare the Fulfill answered.
        request_id: u32,
        /// The claim.
        claim: Vec<u8>,
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
    /// The core changed, as this node computes it
    /// (spec/decisions/promotion-to-core.md).
    CoreChanged,
    /// A peer's kind changed in place, as its status did: it became our
    /// downlink, or a core peer. Its peering, methods and channel stay
    /// (spec/decisions/promotion-to-core.md).
    KindChanged {
        /// The peer.
        peer: NodeId,
        /// Whether it is our downlink now.
        downlink: bool,
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
    /// A peer's link state names, in its offer on a method of our peering,
    /// an account in a settlement hub that its link state held before did
    /// not: the scheme over the hub may look it up at once
    /// (spec/areas/hubs.md#the-members-side). What the peer says is not
    /// taken as true: only the hub says which account a key has.
    HubAccountAdvertised {
        /// The peer.
        peer: NodeId,
        /// The method's scheme.
        scheme: SchemeId,
        /// The hub and the account the offer names.
        details: HubDetails,
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
    /// A peer answered our request to settle with how much it has on its
    /// way to us (spec/decisions/ending-a-peering.md).
    SettlementRequestAnswered {
        /// The node.
        peer: NodeId,
        /// What it has on its way, in the scheme ledger's units.
        settling: u64,
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
    /// As a registrar, something happened to a name or a commitment; or a
    /// name's holder changed in our view (spec/areas/network.md#names).
    Name(NameEvent),
}

/// What happened at a registrar, or to our view of who holds a name
/// (spec/areas/network.md#names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameEvent {
    /// We logged a commitment.
    Committed {
        /// The node that sent it.
        committer: NodeId,
        /// Its position in the log.
        position: u64,
    },
    /// We took the unsealing of a commitment.
    Unsealed {
        /// The name.
        name: Name,
        /// The node named in the commitment.
        holder: NodeId,
        /// Whether it contends for the name: it has not lost to a name
        /// granted already.
        contends: bool,
    },
    /// Its holder renewed a name.
    Renewed {
        /// The name.
        name: Name,
        /// Its holder.
        holder: NodeId,
        /// When it expires now.
        expires_ms: u64,
    },
    /// Windows closed and grace periods ended at our registrar: `granted`
    /// names granted, which we list from now, `lost` unsealed commitments
    /// that lost their names to ones logged earlier, `lapsed` commitments
    /// not unsealed within their windows, and `freed` names whose grace
    /// period ended unrenewed. One event for each step of the registrar's
    /// timer that did any of these, however many it did: a registrar back
    /// after a long downtime may close every window and free every name at
    /// once, and the bounded queue, which drops its oldest, must keep the
    /// events queued before this in the same tick. What we list is
    /// [`PeerNode::registrar`](crate::PeerNode::registrar)'s.
    RegistrarChanged {
        /// Names granted.
        granted: usize,
        /// Unsealed commitments that lost their names.
        lost: usize,
        /// Commitments that lapsed unsealed.
        lapsed: usize,
        /// Names freed.
        freed: usize,
    },
    /// We released names we listed: more than f of the other registrars'
    /// fresh lists lacked their holders for long enough. They are free.
    /// One event for each check that released any, however many it did:
    /// what we list is [`PeerNode::registrar`](crate::PeerNode::registrar)'s.
    Released {
        /// How many names it released.
        names: usize,
    },
    /// In our view, names changed holders: a quorum of the registrars'
    /// lists name a holder for `held` names that had none or another, and
    /// none any more for `unheld` names. One event for each taking of the
    /// view that changed a holder, however many it changed, so that names
    /// never crowd other events out of the bounded queue: the view is
    /// [`PeerNode::holders`](crate::PeerNode::holders).
    HoldersChanged {
        /// Names that have a new holder.
        held: usize,
        /// Names that have none any more.
        unheld: usize,
    },
    /// We started registering a name for ourselves
    /// (spec/areas/network.md#the-registrant).
    Registering {
        /// The name.
        name: Name,
    },
    /// A registrar logged our commitment.
    Receipt {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
        /// The commitment's position in its log.
        position: u64,
    },
    /// Every registrar logged our commitment in time: we unseal it at each.
    Unsealing {
        /// The name.
        name: Name,
    },
    /// A registrar took our unsealing.
    UnsealedAt {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
    },
    /// A registrar refused a commitment, an unsealing or a renewal of ours.
    Refused {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
        /// Its status.
        status: u16,
    },
    /// Not every registrar logged our commitment in time to unseal it: the
    /// registration ends, and no registrar was told the name.
    Abandoned {
        /// The name.
        name: Name,
    },
    /// A registrar answered our renewal of a name we hold.
    RenewedAt {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
        /// The expiry it gives the name now, by its clock.
        expires_ms: u64,
    },
}

/// What the node has counted, for the owner's status
/// (spec/areas/node.md#the-dassie-command) and the simulations.
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
    /// Further settlement methods added to peerings, either side asking.
    pub methods_added: u64,
    /// Settlement methods dropped from peerings that kept another, either
    /// side dropping.
    pub methods_dropped: u64,
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
    /// Commitments to names logged, as a registrar.
    pub commitments_logged: u64,
    /// Commitments that lapsed unsealed.
    pub commitments_lapsed: u64,
    /// Names granted.
    pub names_granted: u64,
    /// Names renewed.
    pub names_renewed: u64,
    /// Names freed after their grace period.
    pub names_freed: u64,
    /// Names released for a split.
    pub names_released: u64,
    /// Registrations of our own started.
    pub registrations_started: u64,
    /// Registrations of our own abandoned, short of a receipt.
    pub registrations_abandoned: u64,
    /// Renewals of our names sent.
    pub renewals_sent: u64,
}
