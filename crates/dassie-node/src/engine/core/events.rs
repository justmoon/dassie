//! What the core reports (D16): everything it would log, as values the
//! driver logs and a simulation records. The core has no logging of its
//! own, so a run can be compared and replayed event by event.

use std::collections::VecDeque;

use dassie_connector::{EndpointId, SpendingCap};
use dassie_identity::NodeId;
use dassie_ledger::{LedgerId, Name, Reference, TransferError};
use dassie_peer::SendError;
use dassie_settlement::{LedgerStatus, SchemeError};

use super::EngineError;
use crate::store::{OutgoingState, PaymentId};

/// Most events kept between two hand-overs: a step's worth, with room for
/// a peer's whole node table arriving at once. Past it, the oldest go, and
/// [`EngineEvent::EventsDropped`] says how many.
pub const MAX_EVENTS: usize = 4096;

/// How much an event matters, which is the level it is logged at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Detail, to follow what happened.
    Debug,
    /// Something the owner may want to know.
    Info,
    /// Something went wrong, and the node copes.
    Warn,
    /// Something went wrong that someone should look at.
    Error,
}

/// What the core reports, by area.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The engine itself.
    Engine(EngineEvent),
    /// The peer node's own events, passed on.
    PeerNode(dassie_peer::Event),
    /// Peers, as the core deals with them.
    Peer(PeerEvent),
    /// BTP tokens and clients.
    Btp(BtpEvent),
    /// Incoming and outgoing payments.
    Payment(PaymentEvent),
    /// Settlement schemes and settlements.
    Settlement(SettlementEvent),
}

/// The engine itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    /// An exchange rate did not fit the table.
    RateTableFull {
        /// The currency.
        code: &'static str,
    },
    /// More events came between two hand-overs than are kept.
    EventsDropped {
        /// How many went.
        count: u64,
    },
    /// A connection had more to send than a poll takes: the rest goes in
    /// the next step.
    SendsLeft,
    /// A step's end took all its rounds: what is left goes in the next.
    RoundsLeft,
}

/// Peers, as the core deals with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerEvent {
    /// A peer got no connector endpoint: its scheme is gone, or its account.
    NoEndpoint {
        /// The peer.
        node: NodeId,
        /// Why.
        error: String,
    },
    /// A peer sent a Prepare with a request ID in flight; it is dropped.
    DuplicateRequest {
        /// The peer's endpoint.
        source: EndpointId,
        /// The request ID.
        request_id: u32,
    },
}

/// BTP tokens and clients.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BtpEvent {
    /// A token was made.
    TokenCreated {
        /// Its ID.
        id: u64,
    },
    /// A token was revoked.
    TokenRevoked {
        /// Its ID.
        id: u64,
    },
    /// A token's spending cap was set or lifted.
    TokenCapSet {
        /// Its ID.
        id: u64,
        /// The cap, if any.
        cap: Option<SpendingCap>,
    },
    /// A client connected.
    ClientConnected {
        /// Its link.
        link: u64,
        /// Its ILP address.
        address: String,
    },
    /// A client went away.
    ClientGone {
        /// Its link.
        link: u64,
    },
}

/// Incoming and outgoing payments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaymentEvent {
    /// More incoming payments were open at start than the bounds allow.
    IncomingOverBounds {
        /// How many were closed.
        dropped: usize,
    },
    /// An incoming payment was made.
    IncomingCreated {
        /// Its ID.
        id: PaymentId,
        /// Its ledger.
        ledger: LedgerId,
    },
    /// An unpaid incoming payment closed to make room for another.
    IncomingClosedForRoom {
        /// Its ID.
        id: PaymentId,
    },
    /// A connection that paid only dust and was idle gave its place in an
    /// incoming payment to a new payer.
    IncomingConnectionReplaced {
        /// The payment.
        id: PaymentId,
    },
    /// An incoming payment received all it asked for.
    IncomingCompleted {
        /// Its ID.
        id: PaymentId,
    },
    /// A receiver's STREAM connection closed.
    StreamClosed {
        /// The connection.
        connection: u64,
        /// The STREAM error code.
        code: dassie_stream::ErrorCode,
        /// Whether the other side closed it.
        by_remote: bool,
    },
    /// An outgoing payment was made.
    OutgoingCreated {
        /// Its ID.
        id: PaymentId,
        /// What it may take.
        debit: u64,
    },
    /// An outgoing payment started sending.
    OutgoingSending {
        /// Its ID.
        id: PaymentId,
        /// What is left to send.
        left: u64,
        /// When.
        at_ms: u64,
    },
    /// An outgoing payment's connection closed early; it connects again.
    OutgoingReconnects {
        /// Its ID.
        id: PaymentId,
        /// Why it closed.
        reason: String,
    },
    /// An outgoing payment finished.
    OutgoingFinished {
        /// Its ID.
        id: PaymentId,
        /// How.
        state: OutgoingState,
        /// What it sent.
        sent: u64,
        /// Why it failed, if it did.
        failure: Option<String>,
    },
}

/// Settlement schemes and settlements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettlementEvent {
    /// A scheme's balance at activation was booked.
    BalanceBooked {
        /// The scheme.
        scheme: Name,
        /// How it was booked.
        reference: Reference,
        /// How much.
        amount: u64,
    },
    /// The underlying ledger differed from the books, which now follow it.
    Reconciled {
        /// The scheme.
        scheme: Name,
        /// By how much.
        amount: u64,
        /// Whether the books gained.
        gain: bool,
    },
    /// A scheme's message to a peer could not be sent.
    MessageNotSent {
        /// The peer.
        peer: NodeId,
        /// Why.
        error: SendError,
    },
    /// A node without accounts here settled with us: booked as a deposit.
    DepositFromStranger {
        /// The node.
        peer: Name,
        /// How much.
        amount: u64,
    },
    /// The books refused a scheme's effects, all of them: it is fed nothing
    /// more.
    SchemeStopped {
        /// The scheme.
        scheme: Name,
        /// Why.
        error: EngineError,
    },
    /// The books cannot take a settlement now.
    BooksCannotSettle {
        /// The peer.
        node: NodeId,
        /// Why.
        error: TransferError,
    },
    /// A settlement was prepared.
    Settling {
        /// The peer.
        node: NodeId,
        /// How much.
        amount: u64,
        /// Its ID.
        id: Name,
    },
    /// A scheme cannot settle now, said now and then.
    SchemeUnavailable {
        /// The peer.
        node: NodeId,
        /// The scheme.
        scheme: Name,
        /// How the scheme stands.
        status: LedgerStatus,
    },
    /// A scheme refused to prepare a settlement.
    PrepareFailed {
        /// The peer.
        node: NodeId,
        /// Why.
        error: SchemeError,
    },
    /// Settlements pending from before a restart are reconciled.
    Reconciling {
        /// The scheme.
        scheme: Name,
        /// How many.
        pending: usize,
    },
    /// A scheme executed a settlement the core does not know.
    UnknownExecuted {
        /// The scheme.
        scheme: Name,
        /// The settlement.
        id: Name,
    },
    /// A settlement was posted, and could not be announced to the peer.
    AnnounceFailed {
        /// The peer.
        peer: NodeId,
        /// Why.
        error: SendError,
    },
    /// A scheme cancelled a settlement the core does not know.
    UnknownCancelled {
        /// The scheme.
        scheme: Name,
        /// The settlement.
        id: Name,
    },
    /// A settlement of ours failed for good.
    Failed {
        /// The peer.
        peer: NodeId,
        /// How much.
        amount: u64,
    },
    /// A peer's settlement message was taken.
    PeerSettled {
        /// The peer.
        from: NodeId,
        /// How much.
        amount: u64,
    },
    /// Our books took a peer's settlement: with its message on the stub,
    /// once its ledger shows it on the XRP Ledger, before or after the
    /// message.
    SettlementBooked {
        /// The peer.
        from: NodeId,
        /// How much.
        amount: u64,
    },
    /// A peer's settlement was refused.
    PeerSettlementRefused {
        /// The peer.
        from: NodeId,
        /// Why.
        error: EngineError,
    },
    /// The fees a peer earned us raised the credit we extend to it (D20).
    CreditRaised {
        /// The peer.
        peer: Name,
        /// The credit now.
        credit: u64,
    },
    /// The fees peers earned us could not be read, or not credited.
    EarningsNotRead {
        /// Why.
        error: EngineError,
    },
}

impl Event {
    /// How much it matters: the level it is logged at, and what a
    /// simulation counts as a failure ([`Severity::Error`]).
    #[must_use]
    pub const fn severity(&self) -> Severity {
        match self {
            Self::Engine(EngineEvent::RateTableFull { .. } | EngineEvent::EventsDropped { .. }) => {
                Severity::Warn
            }
            // Bounds that nothing should reach: someone should look.
            Self::Engine(EngineEvent::SendsLeft | EngineEvent::RoundsLeft) => Severity::Error,
            Self::PeerNode(event) => peer_node_severity(event),
            Self::Peer(event) => match event {
                PeerEvent::DuplicateRequest { .. } => Severity::Debug,
                PeerEvent::NoEndpoint { .. } => Severity::Error,
            },
            Self::Btp(event) => match event {
                BtpEvent::TokenCreated { .. }
                | BtpEvent::TokenRevoked { .. }
                | BtpEvent::TokenCapSet { .. } => Severity::Info,
                BtpEvent::ClientConnected { .. } | BtpEvent::ClientGone { .. } => Severity::Debug,
            },
            Self::Payment(event) => payment_severity(event),
            Self::Settlement(event) => settlement_severity(event),
        }
    }
}

const fn peer_node_severity(event: &dassie_peer::Event) -> Severity {
    use dassie_peer::Event;
    match event {
        Event::Peered { .. }
        | Event::Depeering { .. }
        | Event::Terminating { .. }
        | Event::PeeringEnded { .. }
        | Event::Advertised { short: true, .. } => Severity::Info,
        Event::SettlementRefused { .. } => Severity::Error,
        Event::RoutesChanged
        | Event::NodeAdded(_)
        | Event::NodeRemoved(_)
        | Event::Registered(_)
        | Event::RegisteredWith(_)
        | Event::PacketFailed { .. }
        | Event::PeeringFailed { .. }
        | Event::Advertised { .. }
        | Event::SettlementDelivered { .. }
        | Event::SettlementRequestAnswered { .. }
        | Event::SettlementRequestRefused { .. }
        | Event::SettlementRequestNotDelivered { .. } => Severity::Debug,
    }
}

const fn payment_severity(event: &PaymentEvent) -> Severity {
    match event {
        PaymentEvent::IncomingCreated { .. }
        | PaymentEvent::IncomingClosedForRoom { .. }
        | PaymentEvent::IncomingConnectionReplaced { .. }
        | PaymentEvent::IncomingCompleted { .. }
        | PaymentEvent::StreamClosed { .. }
        | PaymentEvent::OutgoingSending { .. }
        | PaymentEvent::OutgoingReconnects { .. } => Severity::Debug,
        PaymentEvent::OutgoingCreated { .. } | PaymentEvent::OutgoingFinished { .. } => {
            Severity::Info
        }
        PaymentEvent::IncomingOverBounds { .. } => Severity::Warn,
    }
}

const fn settlement_severity(event: &SettlementEvent) -> Severity {
    match event {
        // At every check, for every peer that earned us something.
        SettlementEvent::MessageNotSent { .. }
        | SettlementEvent::CreditRaised { .. }
        | SettlementEvent::SettlementBooked { .. } => Severity::Debug,
        SettlementEvent::BalanceBooked { .. }
        | SettlementEvent::Settling { .. }
        | SettlementEvent::Reconciling { .. }
        | SettlementEvent::PeerSettled { .. } => Severity::Info,
        SettlementEvent::Reconciled { .. }
        | SettlementEvent::DepositFromStranger { .. }
        | SettlementEvent::BooksCannotSettle { .. }
        | SettlementEvent::SchemeUnavailable { .. }
        | SettlementEvent::PrepareFailed { .. }
        | SettlementEvent::UnknownExecuted { .. }
        | SettlementEvent::UnknownCancelled { .. }
        | SettlementEvent::Failed { .. }
        | SettlementEvent::PeerSettlementRefused { .. } => Severity::Warn,
        SettlementEvent::SchemeStopped { .. }
        | SettlementEvent::AnnounceFailed { .. }
        | SettlementEvent::EarningsNotRead { .. } => Severity::Error,
    }
}

impl From<dassie_peer::Event> for Event {
    fn from(event: dassie_peer::Event) -> Self {
        Self::PeerNode(event)
    }
}

impl From<EngineEvent> for Event {
    fn from(event: EngineEvent) -> Self {
        Self::Engine(event)
    }
}

impl From<PeerEvent> for Event {
    fn from(event: PeerEvent) -> Self {
        Self::Peer(event)
    }
}

impl From<BtpEvent> for Event {
    fn from(event: BtpEvent) -> Self {
        Self::Btp(event)
    }
}

impl From<PaymentEvent> for Event {
    fn from(event: PaymentEvent) -> Self {
        Self::Payment(event)
    }
}

impl From<SettlementEvent> for Event {
    fn from(event: SettlementEvent) -> Self {
        Self::Settlement(event)
    }
}

/// The events not handed over yet, at most [`MAX_EVENTS`].
#[derive(Debug)]
pub(super) struct Events {
    queue: VecDeque<Event>,
    dropped: u64,
}

impl Events {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(MAX_EVENTS),
            dropped: 0,
        }
    }

    /// Keeps `event`; the oldest goes when the queue is full.
    pub fn push(&mut self, event: Event) {
        if self.queue.len() >= MAX_EVENTS {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back(event);
    }

    /// The events kept, oldest first, after word of any dropped.
    pub fn take(&mut self) -> impl Iterator<Item = Event> + '_ {
        let dropped = std::mem::take(&mut self.dropped);
        let lost =
            (dropped > 0).then_some(Event::Engine(EngineEvent::EventsDropped { count: dropped }));
        lost.into_iter().chain(self.queue.drain(..))
    }

    pub fn check_invariants(&self) {
        assert!(self.queue.len() <= MAX_EVENTS);
    }
}
