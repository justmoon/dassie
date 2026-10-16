//! Outgoing payments: each ledger's payer, a local endpoint that pays out
//! of the owner's funds, and the STREAM connections that send them (D9,
//! spec 7 §4.5).
//!
//! An outgoing payment sends its debit amount to its receiver on one
//! STREAM connection; at most [`MAX_SENDING`] send at once, and the rest
//! wait their turn. What it sent is what the connector posted, written
//! with the journal row (D17): a packet in flight at a crash was never
//! posted, so a restart sends only the rest, on a new connection with the
//! same credentials. A payment completes once it sent its debit amount,
//! or once the receiver says it has what it was to get, which a hop that
//! lost a packet may have paid for (D17); it fails when the receiver keeps
//! closing it or its deadline passes. It ends
//! only once none of its packets is in flight, so what it says it sent is
//! final. A packet to the node's own receiver waits in a bounded queue the
//! step drains, so nothing recurses.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use dassie_connector::{EndpointId, Forwarded, Outcome};
use dassie_ilp::{Address, Timestamp};
use dassie_ledger::{AccountName, LedgerId, OwnerSub};
use dassie_payments::gnap::Limits;
use dassie_stream::{AssetDetails, Config, Connection, Event, Policy, Rate, Role};

use super::events::{EngineEvent, PaymentEvent};
use super::{Core, EngineError, MAX_SENDS_PER_POLL};
use crate::store::{ForeignAmount, OutgoingProgress, OutgoingRow, OutgoingState, PaymentId};

/// The prefix of a payer's address segment below the node's own.
pub(super) const PAYER_SEGMENT: &str = "pay-";
/// Most outgoing payments sending at once, each on its connection.
pub const MAX_SENDING: usize = 16;
/// Most outgoing payments not yet finished.
pub const MAX_UNFINISHED: usize = 4_096;
/// How long an outgoing payment may take.
pub const OUTGOING_DEADLINE_MS: u64 = 10 * 60 * 1_000;
/// How often a payment opens a new connection after one closed before it
/// was done: an idle timeout while liquidity is short, or a receiver that
/// restarted.
const MAX_RECONNECTS: u32 = 8;
/// The wait before the first new connection; it doubles each time.
const RECONNECT_MIN_MS: u64 = 1_000;
/// Most packets to the node's own receiver waiting, and handled per step.
const MAX_LOCAL_QUEUE: usize = 1_024;
const LOCAL_PER_STEP: usize = 256;

/// What sends outgoing payments: each active ledger's payer, the payments
/// not yet finished, those waiting to start, the rates being measured,
/// the payers' packets in flight, and the packets for the node's own
/// receiver.
#[derive(Default)]
pub(super) struct Spending {
    pub payers: BTreeMap<LedgerId, Payer>,
    pub outgoing: BTreeMap<PaymentId, Unfinished>,
    /// By when each may start, and its ID.
    pub waiting: BTreeSet<(u64, PaymentId)>,
    /// The probes measuring rates, by their ID.
    pub probes: BTreeMap<u64, super::probes::Probe>,
    /// The connector's request ID for each packet in flight, and the
    /// payment or probe and the STREAM request it is.
    pub requests: BTreeMap<u32, (Sender, u64)>,
    pub next_request: u32,
    pub local_queue: VecDeque<(Forwarded, Vec<u8>)>,
    /// Whether a connection had more to send than a poll takes.
    pub sends_left: bool,
}

/// Where the answer to a payer's packet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Answer {
    /// In `out`, given at once by the connector.
    Out(usize),
    /// In `up`, come back from where it went.
    Up(usize),
    /// There is none: the connector refused it.
    Empty,
}

/// What sent a payer's packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sender {
    /// An outgoing payment.
    Payment(PaymentId),
    /// A rate probe.
    Probe(u64),
}

/// A ledger's payer: the endpoint outgoing payments send from.
pub(super) struct Payer {
    pub endpoint: EndpointId,
}

/// An outgoing payment that has not finished.
pub(super) struct Unfinished {
    row: OutgoingRow,
    /// What it sent before its current connection.
    base_sent: u64,
    connection: Option<Box<Connection>>,
    stream: Option<u64>,
    /// Its packets in flight.
    in_flight: u32,
    /// Why it stops, once it does: it fails once nothing is in flight.
    stopping: Option<String>,
    /// Why its connection closed before it was done, if it did.
    closed: Option<String>,
    /// How often it opened a new connection.
    reconnects: u32,
    /// When it may start sending, while it waits to.
    start_at_ms: u64,
}

/// What an outgoing payment is to be.
#[derive(Clone, Debug)]
pub struct NewOutgoing {
    /// The ledger that pays.
    pub ledger: LedgerId,
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The client that creates it; none for the owner.
    pub client: Option<String>,
    /// The grant it spends, and its limits for this wallet address.
    pub grant: Option<(PaymentId, Limits)>,
    /// The quote it pays, if any.
    pub quote_id: Option<PaymentId>,
    /// The incoming payment it pays.
    pub receiver: String,
    /// The receiver's STREAM destination.
    pub destination: String,
    /// The receiver's STREAM shared secret.
    pub shared_secret: [u8; 32],
    /// What it debits at most.
    pub debit_amount: u64,
    /// What the receiver is to get.
    pub receive_amount: ForeignAmount,
    /// The creator's metadata, as JSON.
    pub metadata: Option<String>,
    /// The wallet address it pays, when the owner named one.
    pub recipient: Option<String>,
    /// Whether it sends all its debit amount, rather than stopping once
    /// the receiver has its receive amount.
    pub fixed_debit: bool,
}

/// A new outgoing payment, and what its grant has spent in the window
/// with it: debited, and delivered.
#[derive(Clone, Debug)]
pub struct CreatedOutgoing {
    /// The payment.
    pub row: OutgoingRow,
    /// What the grant spent, if it has one.
    pub grant_spent: Option<(u64, u64)>,
}

/// The owner's account a ledger's payer spends from.
fn pay_account() -> AccountName {
    AccountName::EquityOwnerSub(OwnerSub::Local(dassie_ledger::name!("pay")))
}

/// A ledger's payer address.
pub(super) fn payer_address(own_address: &str, ledger: LedgerId) -> String {
    format!(
        "{own_address}.{PAYER_SEGMENT}{}",
        super::packets::ledger_slug(ledger)
    )
}

impl Core {
    /// Gives a ledger its payer, a local endpoint spending from the
    /// owner's `local/pay` account.
    pub(super) fn add_payer(&mut self, ledger: LedgerId) -> Result<(), EngineError> {
        if self.spending.payers.contains_key(&ledger) {
            return Ok(());
        }
        let endpoint = self.free_local();
        let name = pay_account();
        let account = self
            .ledger
            .open(dassie_ledger::AccountPath::new(ledger, name))
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        let own = self.connector.own_address().to_string();
        self.connector
            .add_endpoint(endpoint, &payer_address(&own, ledger), account, ledger)
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        self.spending.payers.insert(ledger, Payer { endpoint });
        Ok(())
    }

    /// The ledger whose payer is `endpoint`.
    pub(super) fn payer_ledger(&self, endpoint: EndpointId) -> Option<LedgerId> {
        self.spending
            .payers
            .iter()
            .find(|(_, payer)| payer.endpoint == endpoint)
            .map(|(ledger, _)| *ledger)
    }

    /// How many outgoing payments on `ledger` have not finished.
    pub(super) fn unfinished_on(&self, ledger: LedgerId) -> usize {
        self.spending
            .outgoing
            .values()
            .filter(|payment| payment.row.ledger == ledger)
            .count()
    }

    /// Reads the outgoing payments still sending back at a start: each
    /// sends what is left on a new connection.
    pub(super) fn load_outgoing(&mut self) -> Result<(), String> {
        for row in self.store.sending_payments().map_err(|e| e.to_string())? {
            self.spending.waiting.insert((self.now_ms, row.id));
            self.spending.outgoing.insert(
                row.id,
                Unfinished {
                    base_sent: row.sent_amount,
                    row,
                    connection: None,
                    stream: None,
                    in_flight: 0,
                    stopping: None,
                    closed: None,
                    reconnects: 0,
                    start_at_ms: self.now_ms,
                },
            );
        }
        Ok(())
    }

    /// Makes an outgoing payment: checks the funds and the grant's limits,
    /// reserves what it spends of the grant in the window it is made in,
    /// and starts sending.
    ///
    /// # Errors
    ///
    /// The ledger is not active, the funds or the grant do not cover it,
    /// or too many are unfinished.
    pub fn create_outgoing_payment(
        &mut self,
        new: NewOutgoing,
    ) -> Result<CreatedOutgoing, EngineError> {
        let now = self.now_ms();
        if !self.spending.payers.contains_key(&new.ledger) {
            return Err(EngineError::Precondition("the ledger is not active".into()));
        }
        if new.debit_amount == 0 || new.receive_amount.value == 0 {
            return Err(EngineError::Invalid(
                "the amount must be more than nothing".into(),
            ));
        }
        if self.spending.outgoing.len() >= MAX_UNFINISHED {
            return Err(EngineError::Exhausted(
                "too many outgoing payments are unfinished".into(),
            ));
        }
        check_new(&new)?;
        if let Some(quote) = new.quote_id {
            self.check_quote_unpaid(quote)?;
        }
        self.check_funds(new.ledger, new.debit_amount)?;
        let grant = match &new.grant {
            Some((grant, limits)) => Some(self.reserve(*grant, limits, &new, now)?),
            None => None,
        };
        let row = OutgoingRow {
            id: self.new_payment_id(),
            ledger: new.ledger,
            wallet_address: new.wallet_address,
            client: new.client,
            grant: grant.map(|(grant, window, _)| (grant, window)),
            quote_id: new.quote_id,
            receiver: new.receiver,
            destination: new.destination,
            shared_secret: new.shared_secret,
            debit_amount: new.debit_amount,
            receive_amount: new.receive_amount,
            sent_amount: 0,
            state: OutgoingState::Sending,
            failure: None,
            metadata: new.metadata,
            created_at_ms: now,
            deadline_ms: now + OUTGOING_DEADLINE_MS,
            finished_at_ms: None,
            recipient: new.recipient,
            fixed_debit: new.fixed_debit,
        };
        self.batch.outgoing.created.push(row.clone());
        self.spending.waiting.insert((now, row.id));
        self.spending.outgoing.insert(
            row.id,
            Unfinished {
                row: row.clone(),
                base_sent: 0,
                connection: None,
                stream: None,
                in_flight: 0,
                stopping: None,
                closed: None,
                reconnects: 0,
                start_at_ms: now,
            },
        );
        self.report(PaymentEvent::OutgoingCreated {
            id: row.id,
            debit: row.debit_amount,
        });
        self.pump_outgoing(now);
        Ok(CreatedOutgoing {
            row,
            grant_spent: grant.map(|(_, _, spent)| spent),
        })
    }

    /// Whether the owner's funds on `ledger` cover `debit` on top of what
    /// unfinished payments are still to send.
    fn check_funds(&self, ledger: LedgerId, debit: u64) -> Result<(), EngineError> {
        let root = self.account(ledger, AccountName::EquityOwner)?;
        let balance = self
            .ledger
            .group_balance(root)
            .ok_or_else(|| EngineError::Internal("the owner's group is missing".into()))?;
        let available = balance
            .credits_posted
            .saturating_sub(balance.debits_posted)
            .saturating_sub(balance.debits_pending);
        let promised: u128 = self
            .spending
            .outgoing
            .values()
            .filter(|payment| payment.row.ledger == ledger)
            .map(|payment| u128::from(payment.row.debit_amount - payment.row.sent_amount))
            .sum();
        if available < promised + u128::from(debit) {
            return Err(EngineError::Precondition(
                "the funds do not cover the payment".into(),
            ));
        }
        Ok(())
    }

    /// Whether a quote is unpaid: by no payment on disk, none unfinished,
    /// and none made in this step. Checked here, where payments are made
    /// one at a time, so two at once cannot both pay it.
    fn check_quote_unpaid(&self, quote: PaymentId) -> Result<(), EngineError> {
        let pays = |row: &OutgoingRow| row.quote_id == Some(quote);
        let paid = self.store.quote_used(quote)?
            || self
                .spending
                .outgoing
                .values()
                .any(|payment| pays(&payment.row))
            || self.batch.outgoing.created.iter().any(pays);
        if paid {
            return Err(EngineError::Invalid("the quote was paid already".into()));
        }
        Ok(())
    }

    /// Reserves what a payment spends of its grant, in the window of the
    /// grant's interval it is made in: the grant, the window's start, and
    /// what the window spent with it.
    fn reserve(
        &mut self,
        grant: PaymentId,
        limits: &Limits,
        new: &NewOutgoing,
        now: u64,
    ) -> Result<(PaymentId, u64, (u64, u64)), EngineError> {
        let refused = |why: &str| Err(EngineError::Refused(why.to_string()));
        if limits
            .receiver
            .as_ref()
            .is_some_and(|receiver| *receiver != new.receiver)
        {
            return refused("the grant pays another receiver");
        }
        let asset = new.ledger.asset();
        let debit_asset = limits
            .debit_amount
            .as_ref()
            .is_none_or(|limit| limit.same_asset(asset.code, asset.scale));
        let receive_asset = limits.receive_amount.as_ref().is_none_or(|limit| {
            limit.same_asset(
                &new.receive_amount.asset_code,
                new.receive_amount.asset_scale,
            )
        });
        if !debit_asset || !receive_asset {
            return refused("the grant's limits are in another asset");
        }
        let Some((window, _)) = limits.window(now) else {
            return refused("the grant's interval allows no payment now");
        };
        let mut spent = self.store.grant_spend(grant, window)?;
        if let Some(&(debit, receive)) = self.batch.outgoing.spend.get(&(grant, window)) {
            spent = (with_change(spent.0, debit)?, with_change(spent.1, receive)?);
        }
        let (debit, receive) = (new.debit_amount, new.receive_amount.value);
        if !limits.admit(spent, debit, receive) {
            return refused("the payment would exceed the grant's limits");
        }
        let total = (
            with_change(spent.0, i128::from(debit))?,
            with_change(spent.1, i128::from(receive))?,
        );
        let entry = self
            .batch
            .outgoing
            .spend
            .entry((grant, window))
            .or_default();
        entry.0 += i128::from(debit);
        entry.1 += i128::from(receive);
        Ok((grant, window, total))
    }

    /// Starts waiting payments while there is room, sends what their
    /// connections have to send, and ends those that are done.
    pub(super) fn pump_outgoing(&mut self, now: u64) {
        while self
            .spending
            .outgoing
            .values()
            .filter(|p| p.connection.is_some())
            .count()
            < MAX_SENDING
        {
            let Some(&(at, id)) = self.spending.waiting.first().filter(|(at, _)| *at <= now) else {
                break;
            };
            self.spending.waiting.remove(&(at, id));
            self.connect(id, now);
        }
        let active: Vec<PaymentId> = self
            .spending
            .outgoing
            .iter()
            .filter(|(_, payment)| payment.connection.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in active {
            self.transmit_outgoing(id, now);
            self.settle_outgoing(id, now);
        }
    }

    /// Opens a payment's connection, sending what is left of it.
    fn connect(&mut self, id: PaymentId, now: u64) {
        let Some(payment) = self.spending.outgoing.get_mut(&id) else {
            return;
        };
        let ledger = payment.row.ledger;
        let asset = ledger.asset();
        let Ok(remote) = Address::parse(&payment.row.destination) else {
            payment.stopping = Some("the receiver's address is invalid".into());
            return;
        };
        let mut connection = Box::new(Connection::new(&Config {
            role: Role::Client,
            shared_secret: &payment.row.shared_secret,
            local_address: None,
            remote_address: Some(remote),
            local_asset: Some(AssetDetails {
                code: asset.code,
                scale: asset.scale,
            }),
            receipt: None,
            policy: Policy::DEFAULT,
        }));
        // What each packet must deliver at least: the payment's own rate,
        // its quote's, so a worse path fails rather than delivers less.
        let rate = Rate::new(payment.row.receive_amount.value, payment.row.debit_amount)
            .unwrap_or(Rate::ONE);
        connection.set_exchange_rate(rate);
        let left = payment.row.debit_amount - payment.row.sent_amount;
        payment.stream = connection
            .open_stream()
            .ok()
            .filter(|stream| connection.set_send_max(*stream, left).is_ok());
        payment.base_sent = payment.row.sent_amount;
        payment.connection = Some(connection);
        self.report(PaymentEvent::OutgoingSending {
            id,
            left,
            at_ms: now,
        });
    }

    /// Sends what a payment's connection has to send.
    fn transmit_outgoing(&mut self, id: PaymentId, now: u64) {
        let at = Timestamp::from_unix_ms(now);
        for _ in 0..MAX_SENDS_PER_POLL {
            let Self {
                rng,
                spending,
                scratch,
                ..
            } = self;
            let Some(payment) = spending.outgoing.get_mut(&id) else {
                return;
            };
            let ledger = payment.row.ledger;
            let Some(endpoint) = spending.payers.get(&ledger).map(|payer| payer.endpoint) else {
                return;
            };
            let Some(connection) = payment.connection.as_mut() else {
                return;
            };
            let Some(transmit) = connection.poll_transmit(at, rng.as_mut(), scratch) else {
                return;
            };
            payment.in_flight += 1;
            let local = self.spending.next_request;
            self.spending.next_request = local.wrapping_add(1);
            self.spending
                .requests
                .insert(local, (Sender::Payment(id), transmit.request_id));
            if let Some(answer) = self.send_payer_packet(now, endpoint, local, transmit.length) {
                self.take_answer(now, local, answer);
                self.settle_outgoing(id, now);
            }
        }
        self.sends_left();
    }

    /// A connection had more to send than a poll takes: the rest goes in
    /// the next step.
    pub(super) fn sends_left(&mut self) {
        self.spending.sends_left = true;
        self.report(EngineEvent::SendsLeft);
    }

    /// Hands the connector a payer's packet, written into the scratch
    /// buffer. Returns its answer, if the connector gave it at once: the
    /// caller takes it in, and goes on sending (D16: nothing recurses).
    pub(super) fn send_payer_packet(
        &mut self,
        now: u64,
        endpoint: EndpointId,
        local: u32,
        length: usize,
    ) -> Option<Answer> {
        let outcome = self.connector.handle_prepare(
            now,
            endpoint,
            local,
            self.scratch.get(..length).unwrap_or_default(),
            &mut self.ledger,
            &self.rates,
            &mut self.out,
        );
        self.sent_out(now, outcome)
    }

    /// What the connector did with a payer's packet, and its answer if it
    /// has one already.
    fn sent_out(&mut self, now: u64, outcome: Outcome) -> Option<Answer> {
        match outcome {
            Outcome::Forward(forwarded) if matches!(forwarded.to, EndpointId::Local(_)) => {
                // To the node's own receiver: after this step's sending,
                // so nothing recurses.
                if self.spending.local_queue.len() < MAX_LOCAL_QUEUE {
                    let packet = self
                        .out
                        .get(..forwarded.length)
                        .unwrap_or_default()
                        .to_vec();
                    self.spending.local_queue.push_back((forwarded, packet));
                }
                // A full queue leaves the packet to expire.
                None
            }
            Outcome::Forward(forwarded) => {
                self.forward_out(now, forwarded);
                None
            }
            Outcome::Respond(length) => Some(Answer::Out(length)),
            Outcome::Duplicate | Outcome::UnknownSource => Some(Answer::Empty),
        }
    }

    /// The answer to a payer's packet, come later: to its connection, which
    /// then sends what it has to.
    pub(super) fn payer_response(&mut self, now: u64, local: u32, answer: Answer) {
        match self.take_answer(now, local, answer) {
            Some(Sender::Payment(id)) => self.settle_outgoing(id, now),
            Some(Sender::Probe(id)) => {
                self.transmit_probe(id, now);
                self.settle_probe(id, now);
            }
            None => {}
        }
    }

    /// Gives a payer's connection the answer to its packet `local`, read
    /// where it is. Sends nothing. Returns whose packet it was.
    pub(super) fn take_answer(&mut self, now: u64, local: u32, answer: Answer) -> Option<Sender> {
        let (sender, request) = self.spending.requests.remove(&local)?;
        let Self {
            spending,
            work,
            out,
            up,
            ..
        } = self;
        let response = match answer {
            Answer::Out(length) => out.get(..length),
            Answer::Up(length) => up.get(..length),
            Answer::Empty => None,
        };
        let (at, response) = (Timestamp::from_unix_ms(now), response.unwrap_or_default());
        match sender {
            Sender::Payment(id) => {
                let payment = spending.outgoing.get_mut(&id)?;
                assert!(
                    payment.in_flight > 0,
                    "a response to a packet not in flight"
                );
                payment.in_flight -= 1;
                if let Some(connection) = payment.connection.as_mut() {
                    connection.handle_response(at, request, response, work);
                }
            }
            Sender::Probe(id) => {
                let probe = spending.probes.get_mut(&id)?;
                probe
                    .connection
                    .handle_response(at, request, response, work);
            }
        }
        Some(sender)
    }

    /// Takes in what a payment's connection did: what it sent, and whether
    /// it is done, or must connect again.
    fn settle_outgoing(&mut self, id: PaymentId, now: u64) {
        let Some(payment) = self.spending.outgoing.get_mut(&id) else {
            return;
        };
        closed_early(payment);
        let stream = payment
            .stream
            .and_then(|stream| payment.connection.as_ref()?.stream(stream));
        let sent = payment.base_sent + stream.map_or(0, dassie_stream::Stream::total_sent);
        let delivered = stream.map_or(0, dassie_stream::Stream::remote_received);
        assert!(
            sent <= payment.row.debit_amount,
            "a payment sent more than its debit amount"
        );
        // What was sent is on disk: a restart that saw less would send it
        // again.
        assert!(
            sent >= payment.row.sent_amount,
            "a payment's progress went back"
        );
        if sent != payment.row.sent_amount {
            payment.row.sent_amount = sent;
            self.batch
                .outgoing
                .progress
                .insert(id, progress(&payment.row));
        }
        if now >= payment.row.deadline_ms && payment.stopping.is_none() {
            payment.stopping = Some("the payment ran out of time".into());
        }
        // One of a fixed amount to deliver also ends once the receiver has
        // it, which a hop that lost a packet may have paid for (D17), or
        // all of it but less than a unit sent is worth: no packet can be
        // sized to deliver that, as the hops round down. One of a fixed
        // amount to send ends once it delivered what its quote said, and
        // the receiver takes no more: a request it filled. (A receiver
        // also takes no more while packets wait for a peer's verdict, so
        // that alone does not end one.)
        let receive = u128::from(payment.row.receive_amount.value);
        let debit = u128::from(payment.row.debit_amount.max(1));
        let short = receive.saturating_sub(u128::from(delivered));
        let delivered_enough = short == 0 || (delivered > 0 && short * debit < receive);
        let full = stream.is_some_and(|stream| stream.remote_room() == 0);
        let done = sent == payment.row.debit_amount
            || if payment.row.fixed_debit {
                short == 0 && full
            } else {
                delivered_enough
            };
        if (done || payment.stopping.is_some())
            && let Some(connection) = payment.connection.as_mut()
        {
            connection.close(
                Timestamp::from_unix_ms(now),
                dassie_stream::ErrorCode::NO_ERROR,
            );
        }
        if payment.in_flight > 0 {
            return;
        }
        if done {
            self.finish_outgoing(id, now, None);
        } else if let Some(reason) = payment.stopping.clone() {
            self.finish_outgoing(id, now, Some(reason));
        } else if let Some(reason) = payment.closed.take() {
            if payment.reconnects >= MAX_RECONNECTS {
                payment.stopping = Some(reason.clone());
                self.finish_outgoing(id, now, Some(reason));
                return;
            }
            // Again, for what is left, on a new connection, a while later.
            let wait = RECONNECT_MIN_MS << payment.reconnects.min(16);
            payment.reconnects += 1;
            payment.connection = None;
            payment.stream = None;
            payment.base_sent = sent;
            payment.start_at_ms = now.saturating_add(wait);
            self.spending.waiting.insert((payment.start_at_ms, id));
            self.report(PaymentEvent::OutgoingReconnects { id, reason });
        }
    }

    /// Ends a payment with nothing of it in flight: completed, or failed,
    /// releasing what it did not send from its grant's window.
    fn finish_outgoing(&mut self, id: PaymentId, now: u64, failure: Option<String>) {
        let Some(mut payment) = self.spending.outgoing.remove(&id) else {
            return;
        };
        self.spending.waiting.remove(&(payment.start_at_ms, id));
        let row = &mut payment.row;
        row.state = if failure.is_some() {
            OutgoingState::Failed
        } else {
            OutgoingState::Completed
        };
        row.failure = failure;
        row.finished_at_ms = Some(now.max(row.created_at_ms));
        // What it did not send goes back to its grant's window; what it did
        // not deliver too, if it failed, at the payment's own rate.
        if let Some((grant, window)) = row.grant {
            let unsent = row.debit_amount - row.sent_amount;
            let receive = row.receive_amount.value;
            // It sent at most its debit amount, so delivered at most this.
            let delivered = u64::try_from(
                u128::from(row.sent_amount) * u128::from(receive)
                    / u128::from(row.debit_amount.max(1)),
            );
            assert!(
                delivered.is_ok_and(|delivered| delivered <= receive),
                "a payment delivered more than it was to receive"
            );
            let undelivered = match row.state {
                OutgoingState::Failed => receive - delivered.unwrap_or_default(),
                _ => 0,
            };
            if unsent > 0 || undelivered > 0 {
                let entry = self
                    .batch
                    .outgoing
                    .spend
                    .entry((grant, window))
                    .or_default();
                entry.0 -= i128::from(unsent);
                entry.1 -= i128::from(undelivered);
            }
        }
        self.batch.outgoing.progress.insert(id, progress(row));
        let finished = PaymentEvent::OutgoingFinished {
            id,
            state: row.state,
            sent: row.sent_amount,
            failure: row.failure.clone(),
        };
        self.report(finished);
    }

    /// Hands the packets for the node's own receiver to it, a bounded
    /// number per step; the rest wait for the next.
    pub(super) fn drain_local(&mut self, now: u64) {
        for _ in 0..LOCAL_PER_STEP {
            let Some((forwarded, packet)) = self.spending.local_queue.pop_front() else {
                return;
            };
            if let Some(target) = self.out.get_mut(..packet.len()) {
                target.copy_from_slice(&packet);
                self.deliver(now, forwarded);
            }
        }
    }

    /// Runs the connections' timers and the payments' deadlines.
    pub(super) fn outgoing_timers(&mut self, now: u64) {
        let at = Timestamp::from_unix_ms(now);
        let due: Vec<PaymentId> = self
            .spending
            .outgoing
            .iter()
            .filter(|(_, payment)| {
                payment.row.deadline_ms <= now
                    || payment
                        .connection
                        .as_ref()
                        .and_then(|connection| connection.poll_timeout())
                        .is_some_and(|due| due <= at)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(connection) = self
                .spending
                .outgoing
                .get_mut(&id)
                .and_then(|payment| payment.connection.as_mut())
                && connection.poll_timeout().is_some_and(|due| due <= at)
            {
                connection.handle_timeout(at);
            }
            self.transmit_outgoing(id, now);
            self.settle_outgoing(id, now);
        }
    }

    /// The next time an outgoing payment has something to do.
    pub(super) fn outgoing_deadline(&self) -> Option<u64> {
        if !self.spending.local_queue.is_empty() {
            return Some(self.now_ms);
        }
        let room = self
            .spending
            .outgoing
            .values()
            .filter(|p| p.connection.is_some())
            .count()
            < MAX_SENDING;
        let waiting = self
            .spending
            .waiting
            .first()
            .filter(|_| room)
            .map(|(at, _)| (*at).max(self.now_ms));
        self.spending
            .outgoing
            .values()
            .flat_map(|payment| {
                let timer = payment
                    .connection
                    .as_ref()
                    .and_then(|connection| connection.poll_timeout())
                    .map(|at| at.unix_ms);
                let deadline = payment
                    .stopping
                    .is_none()
                    .then_some(payment.row.deadline_ms);
                [deadline, timer]
            })
            .chain([waiting])
            .flatten()
            .min()
    }

    /// Checks the unfinished payments against their bounds.
    pub(super) fn check_outgoing(&self) {
        assert!(
            self.spending.outgoing.len() <= MAX_UNFINISHED,
            "too many unfinished payments"
        );
        let sending = self
            .spending
            .outgoing
            .values()
            .filter(|payment| payment.connection.is_some())
            .count();
        assert!(sending <= MAX_SENDING, "too many payments sending");
        // A payment finishes only with nothing in flight, so every packet
        // in flight is an unfinished payment's, and counted by it.
        let mut in_flight = BTreeMap::<PaymentId, u32>::new();
        for (sender, _) in self.spending.requests.values() {
            if let Sender::Payment(id) = sender {
                assert!(
                    self.spending.outgoing.contains_key(id),
                    "a packet in flight for a finished payment"
                );
                *in_flight.entry(*id).or_default() += 1;
            }
        }
        for (id, payment) in &self.spending.outgoing {
            assert_eq!(
                payment.in_flight,
                in_flight.get(id).copied().unwrap_or_default(),
                "a payment's packets in flight"
            );
            assert!(
                payment.row.sent_amount <= payment.row.debit_amount,
                "a payment sent more than its debit amount"
            );
            if let Some(connection) = &payment.connection {
                connection.check_invariants();
            }
        }
    }
}

/// Notes a connection that closed while its payment still had to send.
fn closed_early(payment: &mut Unfinished) {
    let Some(connection) = payment.connection.as_mut() else {
        return;
    };
    while let Some(event) = connection.poll_event() {
        if let Event::ConnectionClosed { code, by_remote } = event
            && payment.stopping.is_none()
        {
            payment.closed = Some(if by_remote {
                format!("the receiver closed the payment ({code:?})")
            } else {
                format!("the payment's connection closed ({code:?})")
            });
        }
    }
}

/// What a grant's window spent, with a change on top. Refunds can take it
/// below zero only when they outrun what is on disk, which counts as
/// nothing spent, as the store counts it; past `u64::MAX` it would count
/// less than was spent, so that is an error.
fn with_change(spent: u64, change: i128) -> Result<u64, EngineError> {
    u64::try_from((i128::from(spent) + change).max(0))
        .map_err(|_| EngineError::Internal("a grant's spend overflows".into()))
}

/// What the rows of outgoing payments require, checked before anything
/// enters the engine: the receiver's details may come from another
/// wallet.
fn check_new(new: &NewOutgoing) -> Result<(), EngineError> {
    let invalid = |what: &str| Err(EngineError::Invalid(what.to_string()));
    if new.destination.len() > 1_023 || Address::parse(&new.destination).is_err() {
        return invalid("the receiver's ILP address is invalid");
    }
    let urls_valid = [
        Some(&new.receiver),
        Some(&new.wallet_address),
        new.recipient.as_ref(),
    ]
    .iter()
    .flatten()
    .all(|url| (1..=2_048).contains(&url.len()));
    if !urls_valid {
        return invalid("a URL is too long");
    }
    let code = &new.receive_amount.asset_code;
    if code.is_empty() || code.len() > 32 {
        return invalid("the receiver's asset code is invalid");
    }
    let max = dassie_ledger::MAX_TOTAL;
    if new.debit_amount > max || new.receive_amount.value > max {
        return invalid("the amount is out of range");
    }
    if new
        .metadata
        .as_ref()
        .is_some_and(|metadata| metadata.len() > 4_096)
    {
        return invalid("the metadata is too large");
    }
    Ok(())
}

fn progress(row: &OutgoingRow) -> OutgoingProgress {
    OutgoingProgress {
        sent_amount: row.sent_amount,
        state: row.state,
        failure: row.failure.clone(),
        finished_at_ms: row.finished_at_ms,
    }
}
