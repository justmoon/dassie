//! The core's events, logged (spec/decisions/tigerstyle.md): each at the
//! level its severity says, with the target, message and fields the core
//! logged them with when it logged for itself, so filters and the log's
//! redaction work as before.

use dassie_settlement::ReconcileCause;

use super::core::{
    BtpEvent, EngineEvent, Event, PaymentEvent, PeerEvent, SettlementEvent, Severity,
};
use crate::log::target;

/// A `tracing` event at `severity`, whose level must be a constant.
macro_rules! emit {
    ($severity:expr, $($rest:tt)+) => {
        match $severity {
            Severity::Debug => tracing::debug!($($rest)+),
            Severity::Info => tracing::info!($($rest)+),
            Severity::Warn => tracing::warn!($($rest)+),
            Severity::Error => tracing::error!($($rest)+),
        }
    };
}

/// Logs `event`.
pub(super) fn log(event: &Event) {
    let severity = event.severity();
    match event {
        Event::Engine(event) => engine(severity, event),
        Event::PeerNode(event) => peer_node(severity, event),
        Event::Peer(event) => peer(severity, event),
        Event::Btp(event) => btp(severity, event),
        Event::Payment(event) => payment(severity, event),
        Event::Settlement(event) => settlement(severity, event),
    }
}

fn engine(severity: Severity, event: &EngineEvent) {
    match event {
        EngineEvent::RateTableFull { code } => {
            emit!(severity, target: target::ENGINE, code, "no room for an exchange rate");
        }
        EngineEvent::SendsLeft => {
            emit!(severity, target: target::ENGINE, "a connection had more to send than a poll takes");
        }
        EngineEvent::RoundsLeft => {
            emit!(severity, target: target::ENGINE, "a step's end took all its rounds");
        }
        EngineEvent::EventsDropped { count } => {
            emit!(severity, target: target::ENGINE, count, "events were dropped before they were logged");
        }
    }
}

fn peer_node(severity: Severity, event: &dassie_peer::Event) {
    use dassie_peer::Event;
    match event {
        Event::Peered {
            node,
            scheme,
            inbound,
        } => {
            emit!(severity, target: target::PEER, %node, %scheme, inbound, "peered");
        }
        Event::RoutesChanged => emit!(severity, target: target::ROUTING, "routes changed"),
        Event::NodeAdded(node) => emit!(severity, target: target::PEER, %node, "node added"),
        Event::NodeRemoved(node) => emit!(severity, target: target::PEER, %node, "node removed"),
        Event::Registered(node) => emit!(severity, target: target::PEER, %node, "registration"),
        Event::RegisteredWith(node) => {
            emit!(severity, target: target::PEER, %node, "registered with a bootstrap node");
        }
        Event::PacketFailed {
            peer,
            request_id,
            definite,
        } => {
            emit!(severity, target: target::PEER, %peer, request_id, definite, "a packet to a peer failed");
        }
        Event::FulfillReply {
            peer,
            request_id,
            extra,
        } => {
            let bytes = extra.len();
            emit!(severity, target: target::PEER, %peer, request_id, bytes, "the peer's reply to our Fulfill carries a claim");
        }
        Event::PeeringFailed { node, reason } => {
            emit!(severity, target: target::PEER, %node, reason, "peering failed");
        }
        Event::Depeering { peer, reason } => {
            emit!(severity, target: target::PEER, %peer, ?reason, "ending a peering: collecting first, before the peer is told");
        }
        Event::Terminating {
            peer,
            reason,
            by_us,
        } => {
            emit!(severity, target: target::PEER, %peer, ?reason, by_us, "ending a peering");
        }
        Event::PeeringEnded { peer } => {
            emit!(severity, target: target::PEER, %peer, "peering ended");
        }
        Event::Advertised { peer, on, short } => {
            if *short {
                emit!(severity, target: target::PEER, %peer, "withdrawn: its room stayed short of a packet");
            } else {
                emit!(severity, target: target::PEER, %peer, on, "advertised");
            }
        }
        Event::Reachable { peer, reachable } => {
            emit!(severity, target: target::PEER, %peer, reachable, "whether our packets reach the peer: while they do not, its Prepares are refused");
        }
        Event::SettlementDelivered { .. }
        | Event::SettlementRequestAnswered { .. }
        | Event::SettlementRequestRefused { .. }
        | Event::SettlementRequestNotDelivered { .. }
        | Event::SettlementRefused { .. } => peer_settlement(severity, event),
    }
}

/// The peer node's events about settlement messages.
fn peer_settlement(severity: Severity, event: &dassie_peer::Event) {
    use dassie_peer::Event;
    match event {
        Event::SettlementDelivered { peer, sequence } => {
            emit!(severity, target: target::SETTLEMENT, %peer, sequence, "the peer has our settlement");
        }
        Event::SettlementRequestAnswered { peer, settling } => {
            emit!(severity, target: target::SETTLEMENT, %peer, ?settling, "the peer answered our request to settle");
        }
        Event::SettlementRequestRefused { peer } => {
            emit!(severity, target: target::SETTLEMENT, %peer, "the peer is not peered with us yet; it is asked to settle again at the next check");
        }
        Event::SettlementRequestNotDelivered { peer } => {
            emit!(severity, target: target::SETTLEMENT, %peer, "our request to settle did not reach the peer; it is asked again at the next check");
        }
        Event::SettlementRefused {
            peer,
            sequence,
            status,
        } => {
            emit!(severity, target: target::SETTLEMENT, %peer, sequence, status, "the peer refused our settlement; it is kept and sent again later");
        }
        _ => {}
    }
}

fn peer(severity: Severity, event: &PeerEvent) {
    match event {
        PeerEvent::NoEndpoint { node, error } => {
            emit!(severity, target: target::PEER, %node, %error, "the peer has no endpoint");
        }
        PeerEvent::DuplicateRequest { source, request_id } => {
            emit!(severity, target: target::PEER, %source, request_id, "duplicate request ID");
        }
    }
}

fn btp(severity: Severity, event: &BtpEvent) {
    match event {
        BtpEvent::TokenCreated { id } => {
            emit!(severity, target: target::BTP, id, "BTP token created")
        }
        BtpEvent::TokenRevoked { id } => {
            emit!(severity, target: target::BTP, id, "BTP token revoked")
        }
        BtpEvent::TokenCapSet { id, cap } => {
            emit!(severity, target: target::BTP, id, ?cap, "BTP token cap set");
        }
        BtpEvent::ClientConnected { link, address } => {
            emit!(severity, target: target::BTP, link, %address, "BTP client connected");
        }
        BtpEvent::ClientGone { link } => {
            emit!(severity, target: target::BTP, link, "BTP client gone")
        }
    }
}

fn payment(severity: Severity, event: &PaymentEvent) {
    match event {
        PaymentEvent::IncomingOverBounds { dropped } => {
            emit!(severity, target: target::PAYMENT, dropped, "more incoming payments are open than the bounds allow");
        }
        PaymentEvent::IncomingCreated { id, ledger } => {
            emit!(severity, target: target::PAYMENT, %id, %ledger, "incoming payment created");
        }
        PaymentEvent::IncomingClosedForRoom { id } => {
            emit!(severity, target: target::PAYMENT, %id, "an unpaid incoming payment closed to make room");
        }
        PaymentEvent::IncomingConnectionReplaced { id } => {
            emit!(severity, target: target::PAYMENT, %id, "an idle connection gave its place in an incoming payment to a new payer");
        }
        PaymentEvent::IncomingCompleted { id } => {
            emit!(severity, target: target::PAYMENT, %id, "incoming payment completed");
        }
        PaymentEvent::StreamClosed {
            connection,
            code,
            by_remote,
        } => {
            emit!(severity, target: target::PAYMENT, connection, ?code, by_remote, "STREAM connection closed");
        }
        PaymentEvent::OutgoingCreated { id, debit } => {
            emit!(severity, target: target::PAYMENT, %id, debit, "outgoing payment created");
        }
        PaymentEvent::OutgoingSending { id, left, at_ms } => {
            emit!(severity, target: target::PAYMENT, %id, left, at = at_ms, "outgoing payment sending");
        }
        PaymentEvent::OutgoingReconnects { id, reason } => {
            emit!(severity, target: target::PAYMENT, %id, reason, "outgoing payment connects again");
        }
        PaymentEvent::OutgoingFinished {
            id,
            state,
            sent,
            failure,
        } => {
            let state = state.as_str();
            emit!(severity, target: target::PAYMENT, %id, state, sent, ?failure, "outgoing payment finished");
        }
    }
}

/// The schemes, and our settlements as they start.
fn settlement(severity: Severity, event: &SettlementEvent) {
    match event {
        SettlementEvent::BalanceBooked {
            scheme,
            reference,
            amount,
        } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, ?reference, amount, "booked the scheme's balance");
        }
        SettlementEvent::Reconciled {
            scheme,
            amount,
            gain,
            cause: ReconcileCause::Reserve,
        } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, amount, gain, "the underlying ledger's reserve changed: reconciled");
        }
        SettlementEvent::Reconciled {
            scheme,
            amount,
            gain,
            cause: ReconcileCause::Unexplained,
        } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, amount, gain, "the underlying ledger differs from the books: reconciled");
        }
        SettlementEvent::MessageNotSent { peer, error } => {
            emit!(severity, target: target::SETTLEMENT, %peer, ?error, "a scheme message was not sent");
        }
        SettlementEvent::DepositFromStranger { peer, amount } => {
            emit!(severity, target: target::SETTLEMENT, %peer, amount, "a settlement from a node without accounts: booked as a deposit");
        }
        SettlementEvent::SchemeStopped { scheme, error } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, %error, "the books refused a scheme's effects, all of them: it stops");
        }
        SettlementEvent::BooksCannotSettle { node, error } => {
            emit!(severity, target: target::SETTLEMENT, %node, %error, "the books cannot take a settlement now");
        }
        SettlementEvent::Settling { node, amount, id } => {
            emit!(severity, target: target::SETTLEMENT, %node, amount, %id, "settling");
        }
        SettlementEvent::SchemeUnavailable {
            node,
            scheme,
            status,
        } => {
            emit!(severity, target: target::SETTLEMENT, %node, %scheme, ?status, "the scheme cannot settle now: no funds above the reserve, or no ledger");
        }
        SettlementEvent::PrepareFailed { node, error } => {
            emit!(severity, target: target::SETTLEMENT, %node, %error, "the scheme could not prepare a settlement");
        }
        SettlementEvent::Reconciling { scheme, pending } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, pending, "reconciling settlements from before the restart");
        }
        SettlementEvent::UnknownExecuted { .. }
        | SettlementEvent::AnnounceFailed { .. }
        | SettlementEvent::UnknownCancelled { .. }
        | SettlementEvent::Failed { .. }
        | SettlementEvent::PeerSettled { .. }
        | SettlementEvent::SettlementBooked { .. }
        | SettlementEvent::PeerSettlementRefused { .. }
        | SettlementEvent::CreditRaised { .. }
        | SettlementEvent::EarningsNotRead { .. } => settlement_outcome(severity, event),
    }
}

/// What became of settlements, ours and peers'.
fn settlement_outcome(severity: Severity, event: &SettlementEvent) {
    match event {
        SettlementEvent::UnknownExecuted { scheme, id } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, %id, "an unknown settlement executed");
        }
        SettlementEvent::AnnounceFailed { peer, error } => {
            emit!(severity, target: target::SETTLEMENT, %peer, ?error, "a settlement could not be announced");
        }
        SettlementEvent::UnknownCancelled { scheme, id } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, %id, "an unknown settlement was cancelled");
        }
        SettlementEvent::Failed { peer, amount } => {
            emit!(severity, target: target::SETTLEMENT, %peer, amount, "a settlement failed");
        }
        SettlementEvent::PeerSettled { from, amount } => {
            emit!(severity, target: target::SETTLEMENT, %from, amount, "a peer settled");
        }
        SettlementEvent::SettlementBooked { from, amount } => {
            emit!(severity, target: target::SETTLEMENT, %from, amount, "a peer's settlement is booked");
        }
        SettlementEvent::PeerSettlementRefused { from, error } => {
            emit!(severity, target: target::SETTLEMENT, %from, %error, "a settlement was refused");
        }
        SettlementEvent::CreditRaised { peer, credit } => {
            emit!(severity, target: target::SETTLEMENT, %peer, credit, "a peer earned more credit");
        }
        SettlementEvent::EarningsNotRead { error } => {
            emit!(severity, target: target::SETTLEMENT, %error, "the fees peers earned could not be credited");
        }
        // Logged by `settlement`.
        SettlementEvent::BalanceBooked { .. }
        | SettlementEvent::Reconciled { .. }
        | SettlementEvent::MessageNotSent { .. }
        | SettlementEvent::DepositFromStranger { .. }
        | SettlementEvent::SchemeStopped { .. }
        | SettlementEvent::BooksCannotSettle { .. }
        | SettlementEvent::Settling { .. }
        | SettlementEvent::SchemeUnavailable { .. }
        | SettlementEvent::PrepareFailed { .. }
        | SettlementEvent::Reconciling { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use dassie_identity::NodeKey;

    use super::*;
    use crate::log::{LogLevel, capture};

    // An event is logged at the level its severity says, with the target,
    // message and fields it had when the core logged it itself.
    #[test]
    fn events_are_logged_as_the_core_logged_them() {
        let node = NodeKey::from_bytes(&[7; 32]).node_id();
        let lines = capture(|| {
            log(&SettlementEvent::PeerSettled {
                from: node,
                amount: 5,
            }
            .into());
            log(&PeerEvent::NoEndpoint {
                node,
                error: "gone".into(),
            }
            .into());
            log(&dassie_peer::Event::NodeAdded(node).into());
            log(&EngineEvent::EventsDropped { count: 2 }.into());
        });
        let summary: Vec<(LogLevel, &str, &str)> = lines
            .iter()
            .map(|line| (line.level, line.target.as_str(), line.message.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (LogLevel::Info, target::SETTLEMENT, "a peer settled"),
                (LogLevel::Error, target::PEER, "the peer has no endpoint"),
                (LogLevel::Debug, target::PEER, "node added"),
                (
                    LogLevel::Warn,
                    target::ENGINE,
                    "events were dropped before they were logged"
                ),
            ]
        );
        assert_eq!(
            lines[0].fields,
            [
                ("from".to_string(), node.to_string()),
                ("amount".to_string(), "5".to_string())
            ]
        );
    }
}
