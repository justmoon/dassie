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
    /// Most open at once, over all ledgers.
    pub max_open: usize,
    /// Most open at once for one client.
    pub max_open_per_client: usize,
    /// How long one lasts when its creator names no expiry.
    pub default_expiry_ms: u64,
    /// The longest one may last.
    pub max_expiry_ms: u64,
}

impl IncomingPolicy {
    /// The production values.
    pub const DEFAULT: Self = Self {
        max_open: 4096,
        max_open_per_client: 256,
        default_expiry_ms: 24 * 60 * 60 * 1_000,
        max_expiry_ms: 30 * 24 * 60 * 60 * 1_000,
    };
}

/// An open incoming payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OpenIncoming {
    ledger: LedgerId,
    incoming_amount: Option<u64>,
    received: u64,
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

/// Whether a packet for the connection tagged `tag` may into its receiver
/// on `ledger`, and the payment it pays.
pub(super) fn admission(
    incoming: &BTreeMap<PaymentId, OpenIncoming>,
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
                received: payment.received,
            },
            Some(id),
        ),
        _ => refused,
    }
}

impl Core {
    /// Reads the open incoming payments back at a start.
    pub(super) fn load_incoming(&mut self) -> Result<(), String> {
        let rows = self
            .store
            .open_incoming_payments(self.now_ms)
            .map_err(|e| e.to_string())?;
        for row in rows.into_iter().take(self.incoming_policy.max_open) {
            self.incoming.insert(row.id, open_of(&row));
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
        if expires_at_ms <= now || expires_at_ms - now > policy.max_expiry_ms {
            return Err(EngineError::Invalid(
                "the expiry must be in the future, and at most 30 days away".into(),
            ));
        }
        if new
            .incoming_amount
            .is_some_and(|amount| amount == 0 || amount > dassie_ledger::MAX_TOTAL)
        {
            return Err(EngineError::Invalid("the amount is out of range".into()));
        }
        self.make_room(now, new.client.as_deref())?;
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

    /// Frees the slots of expired payments, then checks there is room for
    /// one more, for `client` too.
    fn make_room(&mut self, now: u64, client: Option<&str>) -> Result<(), EngineError> {
        let policy = self.incoming_policy;
        if self.incoming.len() >= policy.max_open {
            self.incoming
                .retain(|_, payment| now < payment.expires_at_ms);
        }
        if self.incoming.len() >= policy.max_open {
            return Err(EngineError::Exhausted(
                "too many incoming payments are open".into(),
            ));
        }
        if let Some(client) = client {
            let open = self
                .incoming
                .values()
                .filter(|payment| payment.client.as_deref() == Some(client))
                .filter(|payment| now < payment.expires_at_ms)
                .count();
            if open >= policy.max_open_per_client {
                return Err(EngineError::Exhausted(
                    "the client has too many incoming payments open".into(),
                ));
            }
        }
        Ok(())
    }

    fn new_payment_id(&mut self) -> PaymentId {
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
        self.incoming.remove(&id);
        row.completed_at_ms = Some(now);
        self.batch.incoming_progress.insert(
            id,
            IncomingProgress {
                received_amount: row.received_amount,
                completed_at_ms: row.completed_at_ms,
            },
        );
        Ok(row)
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
        let complete = payment.incoming_amount == Some(payment.received);
        let progress = IncomingProgress {
            received_amount: payment.received,
            completed_at_ms: complete.then_some(now),
        };
        if complete {
            self.incoming.remove(&id);
            tracing::debug!(target: target::PAYMENT, %id, "incoming payment completed");
        }
        self.batch.incoming_progress.insert(id, progress);
    }

    /// Checks the open payments against their bounds.
    pub(super) fn check_incoming(&self) {
        assert!(
            self.incoming.len() <= self.incoming_policy.max_open,
            "too many open incoming payments"
        );
        for payment in self.incoming.values() {
            assert!(
                payment
                    .incoming_amount
                    .is_none_or(|amount| payment.received < amount),
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
        expires_at_ms: row.expires_at_ms,
        client: row.client.clone(),
    }
}
