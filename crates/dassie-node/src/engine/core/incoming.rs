//! Open Payments' incoming payments, as the money path sees them: which are
//! open, and what each may still receive (D9, spec 7 §4.5).
//!
//! An incoming payment's STREAM credentials carry its ID and a connection
//! key, sealed in the token as the connection's tag. Each read of the
//! payment gets credentials with a key of their own, so each payment that
//! pays it has a connection of its own, and learns what it delivered, not
//! what others did. When a packet arrives, its receiver asks [`admission`]
//! whether the payment is open on that ledger, how much it may still take
//! over all its connections, and what this one received, and the
//! connection takes no more. What it took is added to the payment and the
//! connection in the step that books the packet, and written with its
//! journal row, so the payment never shows money the books lack, nor the
//! other way round.
//!
//! The owner's payments and clients' are bounded apart, so that clients,
//! whom anyone can be, never crowd out the owner's own requests. When the
//! clients' share is full, the oldest of their payments that received
//! nothing is closed to make room: a flood of requests pushes out only
//! other unpaid requests.

use std::collections::BTreeMap;

use dassie_ilp::ErrorCode;
use dassie_ledger::LedgerId;
use dassie_stream::{Admission, Credentials};

use super::events::PaymentEvent;
use super::{Core, EngineError};
use crate::store::{ConnectionKey, IncomingPaid, IncomingProgress, IncomingRow, PaymentId};

/// Most connections that pay one incoming payment: a new one past them is
/// refused.
pub const MAX_CONNECTIONS_PER_PAYMENT: usize = 32;

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
    /// What each connection received: at most
    /// [`MAX_CONNECTIONS_PER_PAYMENT`] of them.
    connections: BTreeMap<ConnectionKey, u64>,
    created_at_ms: u64,
    expires_at_ms: u64,
    client: Option<String>,
}

impl OpenIncoming {
    /// What it may still receive. The books keep totals below `i64::MAX`.
    fn remaining(&self) -> u64 {
        let ceiling = self.incoming_amount.unwrap_or(dassie_ledger::MAX_TOTAL);
        ceiling.saturating_sub(self.received)
    }
}

/// The tag of a connection's credentials: the payment's ID, then the
/// connection's key; the ID alone for those issued before connections had
/// keys.
pub(super) fn connection_tag(payment: PaymentId, key: ConnectionKey) -> Vec<u8> {
    let mut tag = payment.0.to_vec();
    if key != ConnectionKey::LEGACY {
        tag.extend_from_slice(&key.0);
    }
    tag
}

/// The payment and connection a tag names.
fn parse_tag(tag: &[u8]) -> Option<(PaymentId, ConnectionKey)> {
    let (id, key) = match tag.len() {
        16 => (tag, None),
        32 => {
            let (id, key) = tag.split_at(16);
            (id, Some(key))
        }
        _ => return None,
    };
    let id = PaymentId(<[u8; 16]>::try_from(id).ok()?);
    let key = match key {
        None => ConnectionKey::LEGACY,
        Some(key) => ConnectionKey(<[u8; 16]>::try_from(key).ok()?),
    };
    Some((id, key))
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

/// Incoming payments completed lately, with what each connection received:
/// a sender still sending is told what its connection delivered and that
/// no more fits, rather than refused without a word, so it learns what got
/// there, which may be more than it heard of when a Fulfill was lost.
#[derive(Default)]
pub(super) struct Finished {
    payments: BTreeMap<PaymentId, (LedgerId, BTreeMap<ConnectionKey, u64>)>,
    order: std::collections::VecDeque<PaymentId>,
}

impl Finished {
    fn remember(
        &mut self,
        id: PaymentId,
        ledger: LedgerId,
        received: BTreeMap<ConnectionKey, u64>,
    ) {
        assert!(
            received.len() <= MAX_CONNECTIONS_PER_PAYMENT,
            "a payment has too many connections"
        );
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
/// on `ledger`, and the payment and connection it pays. A connection is
/// told what it received, not what the payment did. A new one past
/// [`MAX_CONNECTIONS_PER_PAYMENT`] is refused.
pub(super) fn admission(
    incoming: &BTreeMap<PaymentId, OpenIncoming>,
    finished: &Finished,
    ledger: LedgerId,
    now_ms: u64,
    tag: &[u8],
) -> (Admission, Option<(PaymentId, ConnectionKey)>) {
    let refused = (Admission::Refuse(ErrorCode::F99_APPLICATION_ERROR), None);
    let Some((id, key)) = parse_tag(tag) else {
        return refused;
    };
    match incoming.get(&id) {
        Some(payment) if payment.ledger == ledger && now_ms < payment.expires_at_ms => {
            let counted = payment.connections.get(&key);
            if counted.is_none() && payment.connections.len() >= MAX_CONNECTIONS_PER_PAYMENT {
                return refused;
            }
            (
                Admission::Accept {
                    money_limit: payment.remaining(),
                    received: counted.copied().unwrap_or(0),
                },
                Some((id, key)),
            )
        }
        None => match finished.payments.get(&id) {
            Some((paid_into, received)) if *paid_into == ledger => (
                Admission::Accept {
                    money_limit: 0,
                    received: received.get(&key).copied().unwrap_or(0),
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
            self.report(PaymentEvent::IncomingOverBounds { dropped });
        }
        let most = (policy.max_open_owner + policy.max_open_clients)
            .saturating_mul(MAX_CONNECTIONS_PER_PAYMENT);
        let connections = self
            .store
            .open_incoming_connections(self.now_ms, u32::try_from(most).unwrap_or(u32::MAX))
            .map_err(|e| e.to_string())?;
        for (id, key, received) in connections {
            if let Some(payment) = self.incoming.get_mut(&id)
                && payment.connections.len() < MAX_CONNECTIONS_PER_PAYMENT
            {
                payment.connections.insert(key, received);
            }
        }
        Ok(())
    }

    /// New credentials for an incoming payment on `ledger`: a connection
    /// of its own for whoever reads them.
    ///
    /// # Errors
    ///
    /// The ledger is not active.
    pub fn incoming_credentials(
        &mut self,
        id: PaymentId,
        ledger: LedgerId,
    ) -> Result<Credentials, EngineError> {
        let mut key = [0u8; 16];
        self.rng.fill_bytes(&mut key);
        let tag = connection_tag(id, ConnectionKey::from_random(key));
        let Self { rng, receivers, .. } = self;
        let receiver = receivers
            .get(&ledger)
            .ok_or_else(|| EngineError::Precondition("the ledger is not active".into()))?;
        let token = receiver.server.issue_token(rng.as_mut(), &tag, None);
        Ok(receiver.server.credentials_for(&token))
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
        if !self.receivers.contains_key(&new.ledger) {
            return Err(EngineError::Precondition("the ledger is not active".into()));
        }
        self.make_room(now, new.client.is_none())?;
        let id = self.new_payment_id();
        // Those kept with it are one connection like any other: the
        // owner's wallet shows them, to pay it with STREAM alone.
        let credentials = self.incoming_credentials(id, new.ledger)?;
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
        self.report(PaymentEvent::IncomingCreated {
            id,
            ledger: row.ledger,
        });
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
                    .filter(|(_, payment)| payment.client.is_some() && payment.received == 0)
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
        self.report(PaymentEvent::IncomingClosedForRoom { id: oldest });
        Ok(())
    }

    /// Closes an open payment: it is complete as of now, or as of when it
    /// was made if the clock went back since.
    fn close_incoming(&mut self, id: PaymentId, now: u64) {
        let Some(payment) = self.incoming.remove(&id) else {
            return;
        };
        self.finished_incoming
            .remember(id, payment.ledger, payment.connections.clone());
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
                .remember(id, payment.ledger, payment.connections.clone());
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

    /// Adds what a fulfilled packet brought to its payment and its
    /// connection, completing the payment when it has all it asked for.
    // Panics only on a broken invariant: money is admitted for open payments.
    #[allow(clippy::panic)]
    pub(super) fn incoming_received(&mut self, paid: IncomingPaid) {
        let IncomingPaid {
            payment: id,
            connection,
            amount,
        } = paid;
        let now = self.now_ms();
        let Some(payment) = self.incoming.get_mut(&id) else {
            panic!("money admitted for a payment that is not open");
        };
        assert!(
            amount <= payment.remaining(),
            "a payment took more than it may"
        );
        payment.received += amount;
        let counted = payment.connections.entry(connection).or_default();
        *counted += amount;
        self.batch
            .incoming_connections
            .insert((id, connection), *counted);
        let complete = payment.incoming_amount == Some(payment.received);
        let progress = IncomingProgress {
            received_amount: payment.received,
            completed_at_ms: complete.then_some(now.max(payment.created_at_ms)),
        };
        if complete {
            let ledger = payment.ledger;
            let received = payment.connections.clone();
            self.incoming.remove(&id);
            self.finished_incoming.remember(id, ledger, received);
            self.report(PaymentEvent::IncomingCompleted { id });
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
                    .is_none_or(|amount| payment.received <= amount),
                "a payment took more than it may"
            );
            assert!(
                payment.incoming_amount != Some(payment.received),
                "a completed payment is open"
            );
            assert!(
                payment.connections.len() <= MAX_CONNECTIONS_PER_PAYMENT,
                "a payment has too many connections"
            );
            let received: u64 = payment.connections.values().sum();
            // What connections received before a restart may be counted by
            // the payment alone, if their rows were not all read back.
            assert!(
                received <= payment.received,
                "a payment's connections do not add up"
            );
        }
    }
}

fn open_of(row: &IncomingRow) -> OpenIncoming {
    OpenIncoming {
        ledger: row.ledger,
        incoming_amount: row.incoming_amount,
        received: row.received_amount,
        connections: BTreeMap::new(),
        created_at_ms: row.created_at_ms,
        expires_at_ms: row.expires_at_ms,
        client: row.client.clone(),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        // A packet's tag is whatever its sender put in the STREAM token:
        // only the two lengths the node writes name a connection, and
        // anything else is refused, without a panic (D16).
        #[test]
        fn any_tag_is_read_or_refused(tag in proptest::collection::vec(any::<u8>(), 0..48)) {
            let parsed = parse_tag(&tag);
            prop_assert_eq!(parsed.is_some(), tag.len() == 16 || tag.len() == 32);
            let (admission, paid) = admission(&BTreeMap::new(), &Finished::default(), LedgerId::StubUsd, 0, &tag);
            prop_assert_eq!(admission, Admission::Refuse(ErrorCode::F99_APPLICATION_ERROR));
            prop_assert_eq!(paid, None);
        }

        #[test]
        fn tags_read_back(id in any::<[u8; 16]>(), key in any::<[u8; 16]>()) {
            let (id, key) = (PaymentId(id), ConnectionKey(key));
            prop_assert_eq!(parse_tag(&connection_tag(id, key)), Some((id, key)));
        }
    }
}
