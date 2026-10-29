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
        Event::MethodAdded { .. } | Event::MethodWinding { .. } | Event::MethodDropped { .. } => {
            peer_method(severity, event);
        }
        Event::RoutesChanged
        | Event::CoreChanged
        | Event::OwnAddress { .. }
        | Event::CheckHttps { .. }
        | Event::KindChanged { .. } => peer_core(severity, event),
        Event::NodeAdded(node) => emit!(severity, target: target::PEER, %node, "node added"),
        Event::NodeRemoved(node) => emit!(severity, target: target::PEER, %node, "node removed"),
        Event::Registered(node) => emit!(severity, target: target::PEER, %node, "registration"),
        Event::RegisteredWith(node) => {
            emit!(severity, target: target::PEER, %node, "registered with a bootstrap node");
        }
        Event::PacketFailed { .. } | Event::FulfillReply { .. } => peer_packet(severity, event),
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
        Event::ChannelClosed { peer } => {
            emit!(severity, target: target::PEER, %peer, "settlement channel closed");
        }
        Event::HubAccountAdvertised {
            peer,
            scheme,
            details,
        } => {
            emit!(severity, target: target::PEER, %peer, %scheme, account = details.account, "the peer names its account in the hub: looked up");
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
        Event::Name(event) => peer_names(severity, event),
    }
}

/// The peer node's events about routes, the core and promotion
/// (spec/decisions/promotion-to-core.md).
fn peer_core(severity: Severity, event: &dassie_peer::Event) {
    use dassie_peer::Event;
    match event {
        Event::CheckHttps {
            peer,
            url,
            registration,
        } => {
            emit!(severity, target: target::PEER, %peer, %url, registration, "checking a node's URL over HTTPS");
        }
        Event::KindChanged { peer, downlink } => {
            emit!(severity, target: target::PEER, %peer, downlink, "a peer's kind changed in place");
        }
        Event::RoutesChanged => emit!(severity, target: target::ROUTING, "routes changed"),
        Event::CoreChanged => emit!(severity, target: target::ROUTING, "the core changed"),
        Event::OwnAddress { own: true } => {
            emit!(severity, target: target::ROUTING, "using the node's own address, as a core node");
        }
        Event::OwnAddress { own: false } => {
            emit!(severity, target: target::ROUTING, "using child addresses beneath the uplinks");
        }
        _ => {}
    }
}

/// The peer node's events about names: as a registrar, and its view of who
/// holds each name (spec/areas/network.md#names).
fn peer_names(severity: Severity, event: &dassie_peer::NameEvent) {
    use dassie_peer::NameEvent as Event;
    match event {
        Event::Committed {
            committer,
            position,
        } => {
            emit!(severity, target: target::PEER, %committer, position, "logged a commitment to a name");
        }
        Event::Unsealed {
            name,
            holder,
            contends,
        } => {
            emit!(severity, target: target::PEER, %name, %holder, contends, "a commitment to a name was unsealed");
        }
        Event::Renewed {
            name,
            holder,
            expires_ms,
        } => {
            emit!(severity, target: target::PEER, %name, %holder, expires_ms, "renewed a name");
        }
        Event::RegistrarChanged {
            granted,
            lost,
            lapsed,
            freed,
        } => {
            emit!(severity, target: target::PEER, granted, lost, lapsed, freed, "windows closed and names freed at our registrar: names granted, unsealings that lost, commitments that lapsed, names freed");
        }
        Event::Released { names } => {
            emit!(severity, target: target::PEER, names, "released names the other registrars' lists show split: they are free");
        }
        Event::HoldersChanged { held, unheld } => {
            emit!(severity, target: target::PEER, held, unheld, "names changed holders in our view of the registrars' lists");
        }
        Event::Registering { .. }
        | Event::Receipt { .. }
        | Event::Unsealing { .. }
        | Event::UnsealedAt { .. }
        | Event::Refused { .. }
        | Event::Abandoned { .. }
        | Event::RenewedAt { .. } => own_names(severity, event),
    }
}

/// The peer node's events about the names it registers and renews for
/// itself (spec/areas/network.md#the-registrant).
fn own_names(severity: Severity, event: &dassie_peer::NameEvent) {
    use dassie_peer::NameEvent as Event;
    match event {
        Event::Registering { name } => {
            emit!(severity, target: target::PEER, %name, "registering a name at every registrar");
        }
        Event::Receipt {
            name,
            registrar,
            position,
        } => {
            emit!(severity, target: target::PEER, %name, %registrar, position, "a registrar logged our commitment to a name");
        }
        Event::Unsealing { name } => {
            emit!(severity, target: target::PEER, %name, "every registrar logged our commitment: unsealing it");
        }
        Event::UnsealedAt { name, registrar } => {
            emit!(severity, target: target::PEER, %name, %registrar, "a registrar took our unsealing");
        }
        Event::Refused {
            name,
            registrar,
            status,
        } => {
            emit!(severity, target: target::PEER, %name, %registrar, status, "a registrar refused our request about a name");
        }
        Event::Abandoned { name } => {
            emit!(severity, target: target::PEER, %name, "not every registrar logged our commitment in time: the registration ends, and no registrar learned the name");
        }
        Event::RenewedAt {
            name,
            registrar,
            expires_ms,
        } => {
            emit!(severity, target: target::PEER, %name, %registrar, expires_ms, "a registrar answered our renewal of a name");
        }
        _ => {}
    }
}

/// The peer node's events about the ILP packets we sent a peer.
fn peer_packet(severity: Severity, event: &dassie_peer::Event) {
    use dassie_peer::Event;
    match event {
        Event::PacketFailed {
            peer,
            request_id,
            definite,
            status,
        } => {
            emit!(severity, target: target::PEER, %peer, request_id, definite, ?status, "a packet to a peer failed");
        }
        Event::FulfillReply {
            peer,
            request_id,
            claim,
        } => {
            let bytes = claim.len();
            emit!(severity, target: target::PEER, %peer, request_id, bytes, "the peer's reply to our Fulfill carries a claim");
        }
        _ => {}
    }
}

/// The peer node's events about a peering's settlement methods.
fn peer_method(severity: Severity, event: &dassie_peer::Event) {
    use dassie_peer::Event;
    match event {
        Event::MethodAdded {
            peer,
            scheme,
            inbound,
        } => {
            emit!(severity, target: target::PEER, %peer, %scheme, inbound, "settlement method added");
        }
        Event::MethodWinding { peer, scheme } => {
            emit!(severity, target: target::PEER, %peer, %scheme, "winding a settlement method down: collecting on it before it is dropped");
        }
        Event::MethodDropped {
            peer,
            scheme,
            by_us,
        } => {
            emit!(severity, target: target::PEER, %peer, %scheme, by_us, "settlement method dropped");
        }
        _ => {}
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
            emit!(severity, target: target::SETTLEMENT, %peer, settling, "the peer answered our request to settle");
        }
        Event::SettlementRequestRefused { peer } => {
            emit!(severity, target: target::SETTLEMENT, %peer, "the peer is not peered with us yet; it is asked to settle again at its next Prepare at the trigger, or the next check");
        }
        Event::SettlementRequestNotDelivered { peer } => {
            emit!(severity, target: target::SETTLEMENT, %peer, "our request to settle did not reach the peer; it is asked again at its next Prepare at the trigger, or the next check");
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

/// The schemes as they activate.
fn activation(severity: Severity, event: &SettlementEvent) {
    match event {
        SettlementEvent::BalanceBooked {
            scheme,
            reference,
            amount,
        } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, ?reference, amount, "booked the scheme's balance");
        }
        SettlementEvent::OtherRealm { scheme } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, "the scheme is of another realm than the node's: it does not run");
        }
        SettlementEvent::Unusable { scheme, reason } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, %reason, "the scheme's stored config or state no longer makes a scheme: it does not run, and its books stay as they are");
        }
        SettlementEvent::Disabled { scheme } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, "the scheme does not run in this build: its books and its line stay as they are");
        }
        SettlementEvent::HubEnded { scheme, hub } => {
            let hub = hex_id(hub);
            emit!(severity, target: target::SETTLEMENT, %scheme, %hub, "a quorum of the hub's validators signed its payout set: the hub ended, and the owner may leave it");
        }
        SettlementEvent::HubLeft {
            scheme,
            hub,
            written_off,
        } => {
            let hub = hex_id(hub);
            emit!(severity, target: target::SETTLEMENT, %scheme, %hub, written_off, "left the hub, which ended: its state is let go, and what the books held of the account is written off through suspense; its payout set pays it to the owner book's account");
        }
        SettlementEvent::HubBooksMended { scheme, held, kept } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, held, kept, "the books of the hub's ledger held what the account's checkpoint does not explain, as after a move to another hub by hand: reconciled through suspense");
        }
        // `settlement` hands only these here.
        _ => {}
    }
}

/// The schemes, and our settlements as they start.
// One arm per event, named, so that a new event is never logged by default.
#[allow(clippy::too_many_lines)]
fn settlement(severity: Severity, event: &SettlementEvent) {
    match event {
        SettlementEvent::BalanceBooked { .. }
        | SettlementEvent::OtherRealm { .. }
        | SettlementEvent::Unusable { .. }
        | SettlementEvent::Disabled { .. }
        | SettlementEvent::HubEnded { .. }
        | SettlementEvent::HubLeft { .. }
        | SettlementEvent::HubBooksMended { .. } => activation(severity, event),
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
        SettlementEvent::PeerLoss {
            scheme,
            peer,
            amount,
            cause,
        } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, %peer, amount, ?cause, "a peering over a hub cost more than its hold: booked as a loss");
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
        SettlementEvent::RequestShort { node, scheme } => {
            emit!(severity, target: target::SETTLEMENT, %node, %scheme, "asked to settle with less on hand than the request calls for: the rest follows once the scheme can fund it");
        }
        SettlementEvent::RequestAllotted { node, scheme } => {
            emit!(severity, target: target::SETTLEMENT, %node, %scheme, "asked to settle while the hub's funds are short: what the other peers' reserves need keeps the hold toward this peer from rising further; the rest follows once there is room");
        }
        SettlementEvent::Reconciling { .. }
        | SettlementEvent::UnknownExecuted { .. }
        | SettlementEvent::AnnounceFailed { .. }
        | SettlementEvent::MessageWaits { .. }
        | SettlementEvent::UnknownCancelled { .. }
        | SettlementEvent::Failed { .. }
        | SettlementEvent::PegPaused { .. }
        | SettlementEvent::PegResumed { .. }
        | SettlementEvent::HubCredit { .. } => settlement_outcome(severity, event),
        SettlementEvent::PeerSettled { .. }
        | SettlementEvent::SettlementBooked { .. }
        | SettlementEvent::PeerSettlementRefused { .. }
        | SettlementEvent::PeerSettlementUnbooked { .. } => settled_by_peers(severity, event),
        SettlementEvent::HistoryGap { .. } | SettlementEvent::SettlementUnproven { .. } => {
            unproven(severity, event);
        }
    }
}

/// What an external asset could not prove of its ledger, with why.
fn unproven(severity: Severity, event: &SettlementEvent) {
    let (SettlementEvent::HistoryGap {
        asset,
        schemes,
        total,
    }
    | SettlementEvent::SettlementUnproven {
        asset,
        schemes,
        total,
    }) = event
    else {
        return;
    };
    let schemes = names(schemes);
    if matches!(event, SettlementEvent::HistoryGap { .. }) {
        emit!(severity, target: target::SETTLEMENT, %asset, schemes, total, "the ledger server no longer had history beyond the history bound: what fell in the gap is matched through suspense, without who it was");
    } else {
        emit!(severity, target: target::SETTLEMENT, %asset, schemes, total, "a settlement past its last ledger is proven neither applied nor absent, as when someone else signed for the account: it stays pending, for the owner");
    }
}

/// Names, comma-separated, for a field.
fn names(names: &[dassie_ledger::Name]) -> String {
    let names: Vec<String> = names.iter().map(ToString::to_string).collect();
    names.join(",")
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
        SettlementEvent::MessageWaits { peer, scheme } => {
            emit!(severity, target: target::SETTLEMENT, %peer, %scheme, "a settlement whose message books was posted while another message was on its way: its message waits");
        }
        SettlementEvent::UnknownCancelled { scheme, id } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, %id, "an unknown settlement was cancelled");
        }
        SettlementEvent::Failed { peer, amount } => {
            emit!(severity, target: target::SETTLEMENT, %peer, amount, "a settlement failed");
        }
        SettlementEvent::PegPaused { scheme, why } => {
            let why = why.message();
            emit!(severity, target: target::SETTLEMENT, %scheme, why, "the scheme is paused: it settles nothing, and its ledger converts nothing");
        }
        SettlementEvent::PegResumed { scheme } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, "the token is back on its peg: the scheme runs again");
        }
        SettlementEvent::Reconciling { scheme, pending } => {
            emit!(severity, target: target::SETTLEMENT, %scheme, pending, "reconciling settlements from before the restart");
        }
        SettlementEvent::HubCredit {
            peer,
            credit,
            holds,
        } => {
            emit!(severity, target: target::SETTLEMENT, %peer, credit, holds, "the credit over a hub followed its holds");
        }
        // Logged by `settlement`.
        SettlementEvent::BalanceBooked { .. }
        | SettlementEvent::Reconciled { .. }
        | SettlementEvent::PeerLoss { .. }
        | SettlementEvent::DepositFromStranger { .. }
        | SettlementEvent::OtherRealm { .. }
        | SettlementEvent::Unusable { .. }
        | SettlementEvent::Disabled { .. }
        | SettlementEvent::HubEnded { .. }
        | SettlementEvent::HubLeft { .. }
        | SettlementEvent::HubBooksMended { .. }
        | SettlementEvent::SchemeStopped { .. }
        | SettlementEvent::BooksCannotSettle { .. }
        | SettlementEvent::Settling { .. }
        | SettlementEvent::SchemeUnavailable { .. }
        | SettlementEvent::PrepareFailed { .. }
        | SettlementEvent::RequestShort { .. }
        | SettlementEvent::RequestAllotted { .. }
        | SettlementEvent::HistoryGap { .. }
        | SettlementEvent::SettlementUnproven { .. }
        // Logged by `settled_by_peers`.
        | SettlementEvent::PeerSettled { .. }
        | SettlementEvent::SettlementBooked { .. }
        | SettlementEvent::PeerSettlementRefused { .. }
        | SettlementEvent::PeerSettlementUnbooked { .. } => {}
    }
}

/// What became of peers' settlements.
fn settled_by_peers(severity: Severity, event: &SettlementEvent) {
    match event {
        SettlementEvent::PeerSettled { from, amount } => {
            emit!(severity, target: target::SETTLEMENT, %from, amount, "a peer settled");
        }
        SettlementEvent::SettlementBooked {
            from,
            scheme,
            amount,
        } => {
            emit!(severity, target: target::SETTLEMENT, %from, %scheme, amount, "a peer's settlement is booked");
        }
        SettlementEvent::PeerSettlementRefused {
            from,
            scheme,
            error,
        } => {
            emit!(severity, target: target::SETTLEMENT, %from, %scheme, %error, "a settlement was refused");
        }
        SettlementEvent::PeerSettlementUnbooked {
            from,
            scheme,
            amount,
        } => {
            emit!(severity, target: target::SETTLEMENT, %from, %scheme, amount, "a settlement on a scheme the node does not run was acknowledged, and booked nothing");
        }
        // Logged by `settlement` or `settlement_outcome`.
        _ => {}
    }
}

/// A hub's ID as hex, for the log.
fn hex_id(id: &[u8; 32]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use crate::log::{LogLevel, capture};

    // An event is logged at the level its severity says, with the target,
    // message and fields it had when the core logged it itself.
    #[test]
    fn events_are_logged_as_the_core_logged_them() {
        let node = dassie_identity::TableKeygen::identity_key(&[7; 32]).node_id();
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
    // An external asset's history gap and its settlement proven neither
    // applied nor absent are warnings, with the asset, its schemes, the
    // total and, in the message, why.
    #[test]
    fn what_an_asset_could_not_prove_is_logged_as_a_warning() {
        let asset = dassie_xrpl::account::FIRST;
        let schemes = vec![
            dassie_ledger::Name::new("xrpl-testnet-xrp").unwrap(),
            dassie_ledger::Name::new("xrpl-testnet-dsusd").unwrap(),
        ];
        let lines = capture(|| {
            log(&SettlementEvent::HistoryGap {
                asset,
                schemes: schemes.clone(),
                total: 1,
            }
            .into());
            log(&SettlementEvent::SettlementUnproven {
                asset,
                schemes,
                total: 2,
            }
            .into());
        });
        assert_eq!(lines.len(), 2);
        for (line, total) in lines.iter().zip(["1", "2"]) {
            assert_eq!(
                (line.level, line.target.as_str()),
                (LogLevel::Warn, target::SETTLEMENT)
            );
            assert_eq!(
                line.fields,
                [
                    ("asset".to_string(), asset.to_string()),
                    (
                        "schemes".to_string(),
                        "xrpl-testnet-xrp,xrpl-testnet-dsusd".to_string()
                    ),
                    ("total".to_string(), total.to_string()),
                ]
            );
        }
        assert!(lines[0].message.contains("history"));
        assert!(lines[1].message.contains("neither applied nor absent"));
    }
}
