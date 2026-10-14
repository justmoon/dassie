//! Open Payments' incoming payments, as the money path sees them: which are
//! open, and what each may still receive (D9, spec 7 §4.5).
//!
//! An incoming payment's STREAM credentials carry its ID, sealed in the
//! token as the connection's tag. When a packet arrives, its receiver asks
//! [`admission`] whether the payment is open on that ledger and how much
//! it may still take, and the connection takes no more. What it took is
//! added to the payment in the step that books the packet, and written
//! with its journal row, so the payment never shows money the books lack,
//! nor the other way round.
//!
//! A packet that came from a peer counts only once that peer's verdict
//! says the Fulfill counted there too (D17): until then it waits, still
//! taking its place against what the payment may receive, and a refusal
//! frees that place again.
//!
//! The owner's payments and clients' are bounded apart, so that clients,
//! whom anyone can be, never crowd out the owner's own requests. When the
//! clients' share is full, the oldest of their payments that received
//! nothing is closed to make room: a flood of requests pushes out only
//! other unpaid requests.

use std::collections::BTreeMap;

use dassie_ilp::ErrorCode;
use dassie_ledger::LedgerId;
use dassie_stream::Admission;

use super::{Core, EngineError};
use crate::log::target;
use crate::store::{IncomingProgress, IncomingRow, PaymentId};

/// The bounds on incoming payments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncomingPolicy {
    /// Most of the owner's open at once, over all ledgers.
    pub max_open_owner: usize,
    /// Most of clients' open at once, over all ledgers.
    pub max_open_clients: usize,
    /// How long one lasts when its creator names no expiry.
    pub default_expiry_ms: u64,
    /// The longest one of the owner's may last.
    pub max_expiry_ms: u64,
    /// The longest one of a client's may last.
    pub max_client_expiry_ms: u64,
}

impl IncomingPolicy {
    /// The production values.
    pub const DEFAULT: Self = Self {
        max_open_owner: 1024,
        max_open_clients: 4096,
        default_expiry_ms: 24 * 60 * 60 * 1_000,
        max_expiry_ms: 30 * 24 * 60 * 60 * 1_000,
        max_client_expiry_ms: 7 * 24 * 60 * 60 * 1_000,
    };
}

/// An open incoming payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OpenIncoming {
    ledger: LedgerId,
    incoming_amount: Option<u64>,
    received: u64,
    /// What fulfilled packets brought that waits for a peer's verdict.
    waiting: u64,
    created_at_ms: u64,
    expires_at_ms: u64,
    client: Option<String>,
}

impl OpenIncoming {
    /// What it may still receive. The books keep totals below `i64::MAX`.
    fn remaining(&self) -> u64 {
        let ceiling = self.incoming_amount.unwrap_or(dassie_ledger::MAX_TOTAL);
        ceiling.saturating_sub(self.received + self.waiting)
    }
}

/// What a new incoming payment is to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIncoming {
    /// The ledger it pays into.
    pub ledger: LedgerId,
    /// The wallet address it is created at.
    pub wallet_address: String,
    /// The client creating it; none for the owner.
    pub client: Option<String>,
    /// The most it accepts.
    pub incoming_amount: Option<u64>,
    /// When it stops accepting money; the default when none.
    pub expires_at_ms: Option<u64>,
    /// The creator's metadata, as JSON.
    pub metadata: Option<String>,
}

/// Most completed incoming payments remembered, to tell their senders.
const MAX_FINISHED: usize = 4_096;

/// Incoming payments completed lately, with what they received: a sender
/// still sending is told the total and that no more fits, rather than
/// refused without a word, so it learns the payment got what it asked
/// for, which a hop that lost a packet may have paid for (D17).
#[derive(Default)]
pub(super) struct Finished {
    payments: BTreeMap<PaymentId, (LedgerId, u64)>,
    order: std::collections::VecDeque<PaymentId>,
}

impl Finished {
    fn remember(&mut self, id: PaymentId, ledger: LedgerId, received: u64) {
        if self.payments.insert(id, (ledger, received)).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > MAX_FINISHED {
            if let Some(oldest) = self.order.pop_front() {
                self.payments.remove(&oldest);
            }
        }
    }
}

/// Whether a packet for the connection tagged `tag` may into its receiver
/// on `ledger`, and the payment it pays.
pub(super) fn admission(
    incoming: &BTreeMap<PaymentId, OpenIncoming>,
    finished: &Finished,
    ledger: LedgerId,
    now_ms: u64,
    tag: &[u8],
) -> (Admission, Option<PaymentId>) {
    let refused = (Admission::Refuse(ErrorCode::F99_APPLICATION_ERROR), None);
    let Ok(id) = <[u8; 16]>::try_from(tag).map(PaymentId) else {
        return refused;
    };
    match incoming.get(&id) {
        Some(payment) if payment.ledger == ledger && now_ms < payment.expires_at_ms => (
            Admission::Accept {
                money_limit: payment.remaining(),
                // What it counts: not what waits for a peer's verdict, which
                // a refusal may take back (D17).
                received: payment.received,
            },
            Some(id),
        ),
        None => match finished.payments.get(&id) {
            Some(&(paid_into, received)) if paid_into == ledger => (
                Admission::Accept {
                    money_limit: 0,
                    received,
                },
                None,
            ),
            _ => refused,
        },
        Some(_) => refused,
    }
}

impl Core {
    /// Reads the open incoming payments back at a start.
    pub(super) fn load_incoming(&mut self) -> Result<(), String> {
        let rows = self
            .store
            .open_incoming_payments(self.now_ms)
            .map_err(|e| e.to_string())?;
        let policy = self.incoming_policy;
        let (mut owner, mut clients, mut dropped) = (0usize, 0usize, 0usize);
        for row in rows {
            let count = if row.client.is_none() {
                &mut owner
            } else {
                &mut clients
            };
            let limit = if row.client.is_none() {
                policy.max_open_owner
            } else {
                policy.max_open_clients
            };
            if *count >= limit {
                dropped += 1;
                continue;
            }
            *count += 1;
            self.incoming.insert(row.id, open_of(&row));
        }
        if dropped > 0 {
            // Only after the bounds were lowered: they take no more money.
            tracing::warn!(target: target::PAYMENT, dropped, "more incoming payments are open than the bounds allow");
        }
        Ok(())
    }

    /// Creates an incoming payment on an active ledger.
    ///
    /// # Errors
    ///
    /// The ledger is not active, the expiry or amount is out of range, or
    /// too many are open.
    pub fn create_incoming_payment(
        &mut self,
        new: NewIncoming,
    ) -> Result<IncomingRow, EngineError> {
        let now = self.now_ms();
        let policy = self.incoming_policy;
        let expires_at_ms = new
            .expires_at_ms
            .unwrap_or(now.saturating_add(policy.default_expiry_ms));
        let (longest, days) = match new.client {
            None => (policy.max_expiry_ms, "30"),
            Some(_) => (policy.max_client_expiry_ms, "7"),
        };
        if expires_at_ms <= now || expires_at_ms - now > longest {
            return Err(EngineError::Invalid(format!(
                "the expiry must be in the future, and at most {days} days away"
            )));
        }
        if new
            .incoming_amount
            .is_some_and(|amount| amount == 0 || amount > dassie_ledger::MAX_TOTAL)
        {
            return Err(EngineError::Invalid("the amount is out of range".into()));
        }
        self.make_room(now, new.client.is_none())?;
        let id = self.new_payment_id();
        let Self { rng, receivers, .. } = self;
        let receiver = receivers
            .get(&new.ledger)
            .ok_or_else(|| EngineError::Precondition("the ledger is not active".into()))?;
        let token = receiver.server.issue_token(rng.as_mut(), &id.0, None);
        let credentials = receiver.server.credentials_for(&token);
        let row = IncomingRow {
            id,
            ledger: new.ledger,
            wallet_address: new.wallet_address,
            client: new.client,
            incoming_amount: new.incoming_amount,
            received_amount: 0,
            ilp_address: credentials.destination,
            shared_secret: credentials.shared_secret,
            metadata: new.metadata,
            created_at_ms: now,
            expires_at_ms,
            completed_at_ms: None,
        };
        self.incoming.insert(id, open_of(&row));
        self.batch.incoming_created.push(row.clone());
        tracing::debug!(target: target::PAYMENT, %id, ledger = %row.ledger, "incoming payment created");
        Ok(row)
    }

    /// How many of the owner's, or of clients', are open.
    fn open_count(&self, owner: bool) -> usize {
        self.incoming
            .values()
            .filter(|payment| payment.client.is_none() == owner)
            .count()
    }

    /// Makes room for one more of the owner's or of a client's: forgets
    /// expired payments, and for a client closes the oldest client payment
    /// that received nothing, if need be.
    fn make_room(&mut self, now: u64, owner: bool) -> Result<(), EngineError> {
        let policy = self.incoming_policy;
        let limit = if owner {
            policy.max_open_owner
        } else {
            policy.max_open_clients
        };
        if self.open_count(owner) >= limit {
            self.incoming
                .retain(|_, payment| now < payment.expires_at_ms);
        }
        if self.open_count(owner) < limit {
            return Ok(());
        }
        let unpaid = (!owner)
            .then(|| {
                self.incoming
                    .iter()
                    .filter(|(_, payment)| {
                        payment.client.is_some() && payment.received == 0 && payment.waiting == 0
                    })
                    .min_by_key(|(id, payment)| (payment.created_at_ms, **id))
                    .map(|(id, _)| *id)
            })
            .flatten();
        let Some(oldest) = unpaid else {
            return Err(EngineError::Exhausted(
                "too many incoming payments are open".into(),
            ));
        };
        self.close_incoming(oldest, now);
        tracing::debug!(target: target::PAYMENT, id = %oldest, "an unpaid incoming payment closed to make room");
        Ok(())
    }

    /// Closes an open payment: it is complete as of now, or as of when it
    /// was made if the clock went back since.
    fn close_incoming(&mut self, id: PaymentId, now: u64) {
        let Some(payment) = self.incoming.remove(&id) else {
            return;
        };
        self.finished_incoming
            .remember(id, payment.ledger, payment.received);
        self.batch.incoming_progress.insert(
            id,
            IncomingProgress {
                received_amount: payment.received,
                completed_at_ms: Some(now.max(payment.created_at_ms)),
            },
        );
    }

    pub(super) fn new_payment_id(&mut self) -> PaymentId {
        let mut bytes = [0u8; 16];
        self.rng.fill_bytes(&mut bytes);
        PaymentId::from_random(bytes)
    }

    /// Marks an incoming payment completed: it takes no more money.
    /// Completing a completed one changes nothing.
    ///
    /// # Errors
    ///
    /// It does not exist, or has expired.
    pub fn complete_incoming_payment(&mut self, id: PaymentId) -> Result<IncomingRow, EngineError> {
        let now = self.now_ms();
        let created = self
            .batch
            .incoming_created
            .iter()
            .find(|row| row.id == id)
            .cloned();
        let mut row = match created {
            Some(row) => row,
            None => self
                .store
                .incoming_payment(id)?
                .ok_or_else(|| EngineError::NotFound("no such incoming payment".into()))?,
        };
        // What this batch paid in is not on disk yet.
        if let Some(progress) = self.batch.incoming_progress.get(&id) {
            row.received_amount = progress.received_amount;
            row.completed_at_ms = progress.completed_at_ms;
        }
        if row.completed_at_ms.is_some() {
            return Ok(row);
        }
        if now >= row.expires_at_ms {
            return Err(EngineError::Precondition(
                "the incoming payment has expired".into(),
            ));
        }
        if let Some(payment) = self.incoming.remove(&id) {
            self.finished_incoming
                .remember(id, payment.ledger, payment.received);
        }
        // A clock that went back since it was made must not date its end
        // before its start.
        row.completed_at_ms = Some(now.max(row.created_at_ms));
        self.batch.incoming_progress.insert(
            id,
            IncomingProgress {
                received_amount: row.received_amount,
                completed_at_ms: row.completed_at_ms,
            },
        );
        Ok(row)
    }

    /// Counts what a fulfilled packet from a peer brought as waiting for
    /// the peer's verdict, if the payment is still open.
    pub(super) fn incoming_waiting(&mut self, id: PaymentId, amount: u64) {
        if let Some(payment) = self.incoming.get_mut(&id) {
            assert!(
                amount <= payment.remaining(),
                "a payment took more than it may"
            );
            payment.waiting += amount;
        }
    }

    /// The peer's verdict counted what was waiting: it is received. A
    /// payment closed since, by its client or its expiry, still gets it.
    pub(super) fn incoming_confirmed(&mut self, id: PaymentId, amount: u64) {
        if let Some(payment) = self.incoming.get_mut(&id) {
            assert!(payment.waiting >= amount, "confirmed more than waited");
            payment.waiting -= amount;
            self.incoming_received(id, amount);
            return;
        }
        let row = match self.batch.incoming_progress.get(&id) {
            Some(progress) => Some((progress.received_amount, progress.completed_at_ms)),
            None => self
                .store
                .incoming_payment(id)
                .ok()
                .flatten()
                .map(|row| (row.received_amount, row.completed_at_ms)),
        };
        let Some((received, completed_at_ms)) = row else {
            tracing::error!(target: target::PAYMENT, %id, "a confirmed payment is gone");
            return;
        };
        self.batch.incoming_progress.insert(
            id,
            IncomingProgress {
                received_amount: received.saturating_add(amount),
                completed_at_ms,
            },
        );
    }

    /// The peer refused: what was waiting is free to arrive again. The
    /// payment's STREAM connections counted it, and would tell senders
    /// more arrived than did: they are closed, and a sender starts anew.
    pub(super) fn incoming_released(&mut self, id: PaymentId, amount: u64) {
        let Some(payment) = self.incoming.get_mut(&id) else {
            return;
        };
        assert!(payment.waiting >= amount, "released more than waited");
        payment.waiting -= amount;
        let at = dassie_ilp::Timestamp::from_unix_ms(self.now_ms);
        let Some(receiver) = self.receivers.get_mut(&payment.ledger) else {
            return;
        };
        let tagged: Vec<dassie_stream::ConnectionId> = receiver
            .server
            .connections()
            .map(|(connection, _)| connection)
            .filter(|connection| receiver.server.connection_tag(*connection) == Some(&id.0[..]))
            .collect();
        for connection in tagged {
            if let Some(connection) = receiver.server.connection_mut(connection) {
                connection.close(at, dassie_stream::ErrorCode::NO_ERROR);
            }
        }
    }

    /// Adds what a fulfilled packet brought to its payment, completing it
    /// when it has all it asked for.
    pub(super) fn incoming_received(&mut self, id: PaymentId, amount: u64) {
        let now = self.now_ms();
        let Some(payment) = self.incoming.get_mut(&id) else {
            panic!("money admitted for a payment that is not open");
        };
        assert!(
            amount <= payment.remaining(),
            "a payment took more than it may"
        );
        payment.received += amount;
        let complete = payment.incoming_amount == Some(payment.received) && payment.waiting == 0;
        let progress = IncomingProgress {
            received_amount: payment.received,
            completed_at_ms: complete.then_some(now.max(payment.created_at_ms)),
        };
        if complete {
            let ledger = payment.ledger;
            let received = payment.received;
            self.incoming.remove(&id);
            self.finished_incoming.remember(id, ledger, received);
            tracing::debug!(target: target::PAYMENT, %id, "incoming payment completed");
        }
        self.batch.incoming_progress.insert(id, progress);
    }

    /// Checks the open payments against their bounds.
    pub(super) fn check_incoming(&self) {
        let policy = self.incoming_policy;
        assert!(
            self.open_count(true) <= policy.max_open_owner
                && self.open_count(false) <= policy.max_open_clients,
            "too many open incoming payments"
        );
        for payment in self.incoming.values() {
            assert!(
                payment
                    .incoming_amount
                    .is_none_or(|amount| payment.received + payment.waiting <= amount),
                "a payment took more than it may"
            );
            assert!(
                payment.incoming_amount != Some(payment.received),
                "a completed payment is open"
            );
        }
    }
}

fn open_of(row: &IncomingRow) -> OpenIncoming {
    OpenIncoming {
        ledger: row.ledger,
        incoming_amount: row.incoming_amount,
        received: row.received_amount,
        waiting: 0,
        created_at_ms: row.created_at_ms,
        expires_at_ms: row.expires_at_ms,
        client: row.client.clone(),
    }
}
