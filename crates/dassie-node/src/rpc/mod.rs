//! The RPC contract with the owner's clients (`proto/dassie/owner/v1`,
//! spec/decisions/rpc-contract.md).
//!
//! [`connection::serve`] runs one connection, over the WebSocket or the IPC
//! socket alike; [`handlers`] answers each method. The original's RPC
//! reported failures as `"RpcFailure"`, as `"Internal server error"`, or as
//! a *successful* response carrying a serialized failure. Here every
//! failure is an `Error` with a code the client can act on and a message
//! that says what happened; only bugs are `INTERNAL`.

pub mod connection;
mod convert;
mod founding;
mod grants;
mod handlers;
mod hub;
mod inspect;
mod ledger;
mod live;
mod names;
mod network;
mod payments;
mod rates;
mod send;

// The records the event log writes are the live streams' own.
pub(crate) use hub::hub_event as hub_event_record;
#[cfg(test)]
pub(crate) use hub::unary as hub_unary;
pub(crate) use ledger::packet as packet_record;

use std::fmt;

use dassie_identity::NodeId;
use dassie_node_core::session::SessionId;

/// The generated messages.
#[allow(missing_docs, clippy::all, clippy::pedantic)]
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/dassie.owner.v1.rs"));
}

/// The WebSocket subprotocol of this contract.
pub const SUBPROTOCOL: &str = "dassie.owner.v1";

/// Largest message, in bytes, either way.
pub const MAX_MESSAGE_BYTES: usize = 1 << 20;

/// Most calls a connection may have in flight.
pub const MAX_CALLS_IN_FLIGHT: usize = 16;

/// Most live streams a connection may hold, among its calls.
pub const MAX_STREAMS_IN_FLIGHT: usize = 12;

/// Whether a method is a live stream, which stays open.
#[must_use]
pub const fn is_live(method: &proto::request::Method) -> bool {
    use proto::request::Method;
    matches!(
        method,
        Method::SubscribeLedgers(_)
            | Method::SubscribePackets(_)
            | Method::SubscribeHubEvents(_)
            | Method::SubscribeTransfers(_)
            | Method::SubscribeNodes(_)
            | Method::SubscribeRoutes(_)
            | Method::SubscribeRegistration(_)
            | Method::SubscribeLogs(_)
            | Method::SubscribeIncomingPayments(_)
            | Method::SubscribeOutgoingPayments(_)
            | Method::SubscribeGrants(_)
            | Method::SubscribeOutgoingPayment(_)
            | Method::SubscribeNames(_)
    )
}

/// Who is on the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The IPC socket: whoever can open it is the owner.
    Owner,
    /// A browser, with the session its cookie named when it connected.
    Browser {
        /// The session.
        session: Option<SessionId>,
        /// Whether it connected from a loopback address: only then may it
        /// start a pairing (spec/areas/rpc.md#paired-devices).
        loopback: bool,
    },
    /// An owner app on a stream with purpose `owner`, with the device key
    /// its TLS proved. Authenticated while the device is paired.
    Device(NodeId),
}

/// A call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcError {
    /// What kind of failure.
    pub code: proto::ErrorCode,
    /// For people.
    pub message: String,
}

impl RpcError {
    fn new(code: proto::ErrorCode, message: impl fmt::Display) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    /// The call needs a session.
    #[must_use]
    pub fn unauthenticated() -> Self {
        Self::new(proto::ErrorCode::Unauthenticated, "log in first")
    }

    fn invalid(message: impl fmt::Display) -> Self {
        Self::new(proto::ErrorCode::InvalidArgument, message)
    }

    fn precondition(message: impl fmt::Display) -> Self {
        Self::new(proto::ErrorCode::FailedPrecondition, message)
    }

    fn not_found(message: impl fmt::Display) -> Self {
        Self::new(proto::ErrorCode::NotFound, message)
    }

    fn unavailable(message: impl fmt::Display) -> Self {
        Self::new(proto::ErrorCode::Unavailable, message)
    }

    fn exhausted(message: impl fmt::Display) -> Self {
        Self::new(proto::ErrorCode::ResourceExhausted, message)
    }

    /// A bug. The details go to the log, not to the client.
    fn internal(error: impl fmt::Display) -> Self {
        tracing::error!(target: crate::log::target::RPC, %error, "internal error in an RPC call");
        Self::new(
            proto::ErrorCode::Internal,
            "internal error; see the node's log",
        )
    }
}
