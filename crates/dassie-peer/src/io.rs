//! What goes in and out of the peer node: requests for the driver to send,
//! their results, replies to inbound requests, and events.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::wire::{Kind, SchemeId};

/// Our number for a request we send.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// The driver's number for a request it received, so a reply that comes
/// later finds its way back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InboundId(pub u64);

/// A request for the driver to send: `POST {url}/peer` with `body`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// Its number, for the result.
    pub id: RequestId,
    /// The node it goes to.
    pub to: NodeId,
    /// The node's URL.
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

/// A reply to an inbound request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// The HTTP status (spec 7 §4.7).
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
    /// An ILP packet from a peer, and the reply that acknowledges it.
    Packet {
        /// The peer.
        from: NodeId,
        /// The ILP request ID.
        request_id: u32,
        /// The ILP packet.
        packet: &'a [u8],
        /// The acknowledgement.
        reply: Reply,
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
    /// A peering attempt failed.
    PeeringFailed {
        /// The node asked.
        node: NodeId,
        /// Why, for people.
        reason: &'static str,
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
}
