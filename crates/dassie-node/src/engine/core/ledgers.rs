//! Settlement schemes and the ledgers they serve.

use dassie_ledger::{
    AccountId, AccountName, AccountPath, LedgerId, Name, Realm, Reference, TransferSpec,
};
use dassie_settlement::{
    Activation, AssetId, Detach, Effect, LedgerStatus, LossCause, PegPause, PegView, SchemeError,
    SettlementScheme, activation,
};

use super::Core;
use super::EngineError;
use super::events::SettlementEvent;
use super::packets::{Receiver, receiver_address};
use crate::store::{Checkpoint, Store};

/// An active scheme.
pub(super) struct ActiveScheme {
    pub scheme: Box<dyn SettlementScheme>,
    /// The external asset it settles through, if any.
    pub asset: Option<AssetId>,
    /// The state version last written with a checkpoint.
    pub version: u64,
    /// Why it stopped, if the books could not take what it said: it is fed
    /// nothing more until the node starts again.
    pub failed: Option<String>,
    /// Whether its peerings are over a settlement hub: it signs and takes
    /// promises per packet ([`SettlementScheme::commitments`]).
    pub over_hub: bool,
    /// Why its peg guard paused it, as the owner was last told.
    pub paused: Option<PegPause>,
    /// Whether it converted when the prices were last built
    /// ([`ActiveScheme::converts_now`]).
    pub converts: bool,
}

impl ActiveScheme {
    /// Whether the connector converts to and from its ledger now
    /// ([`SettlementScheme::converts`]). A dollar token's scheme the books
    /// stopped does not: its peg guard is fed nothing more, so it can no
    /// longer tell a token off its peg (spec/decisions/issued-assets.md).
    pub fn converts_now(&self) -> bool {
        self.scheme.converts() && (self.failed.is_none() || self.scheme.peg().is_none())
    }
}

/// A ledger as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerView {
    /// The ledger.
    pub id: LedgerId,
    /// The scheme serving it.
    pub scheme: String,
    /// The owner's posted total: the pool and its sub-accounts.
    pub owner_balance: i128,
    /// What packets in flight hold of it.
    pub owner_pending: u128,
    /// The scheme's balance on the underlying ledger, as the books have
    /// it: less what the ledger holds back.
    pub on_ledger_balance: u64,
    /// What the underlying ledger holds back, never spendable.
    pub reserve: u64,
    /// Where the owner sends money to fund it, if anywhere: its asset's
    /// address.
    pub deposit_address: Option<String>,
    /// Who issues what it settles in, for a token: an address on its
    /// underlying ledger.
    pub issuer: Option<String>,
    /// Whether the node can ask a faucet for test money on its asset.
    pub faucet: bool,
    /// How the scheme stands with its ledger.
    pub status: LedgerStatus,
    /// What its peg guard says, for a dollar token's scheme that has one.
    pub peg: Option<PegView>,
    /// What keeps the scheme from being removed now.
    pub removal_blockers: Vec<RemovalBlocker>,
}

/// What keeps a scheme from being removed, with how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalBlocker {
    /// BTP tokens use its ledger.
    BtpTokens(u32),
    /// Peers settle with it, or are ending a peering that does.
    Peers(u32),
    /// Outgoing payments from its ledger have not finished.
    OutgoingPayments(u32),
}

impl RemovalBlocker {
    /// The reason, for people.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::BtpTokens(_) => "BTP tokens use its ledger",
            Self::Peers(_) => "peers settle with it",
            Self::OutgoingPayments(_) => "outgoing payments from its ledger have not finished",
        }
    }
}

/// The accounts every active ledger has
/// (spec/areas/ledger.md#chart-of-accounts).
const COMMON_ACCOUNTS: [AccountName; 8] = [
    AccountName::AssetsSettlement,
    AccountName::EquityOwner,
    AccountName::EquitySuspense,
    AccountName::RevenueFx,
    AccountName::RevenueFees,
    AccountName::ExpensesFx,
    AccountName::ExpensesFees,
    AccountName::ExpensesLosses,
];

/// Most accounts the node opens for itself, never a peer's: one for every
/// BTP token ever made, and on every ledger its common accounts and its
/// payer's. Peers' accounts, which stay when their channels close, leave
/// room for these (spec/decisions/closing-channels.md).
pub(super) const OWN_ACCOUNTS: usize =
    super::tokens::MAX_TOKEN_ROWS + LedgerId::ALL.len() * (COMMON_ACCOUNTS.len() + 1);

impl Core {
    /// Activates the stored schemes, booking what changed while the node was
    /// down. One of another realm than the node's, which the node's realm
    /// was changed past, is left as it is, and does not run: the owner is
    /// told, and the node starts with the rest
    /// (spec/decisions/mainnet-schemes.md). So is a disabled one, such as
    /// test RLUSD: its books and its line stay as they are, and nothing is
    /// written off (spec/decisions/issued-assets.md). The peerings on a
    /// scheme left out are ended, collecting nothing, by the peer
    /// protocol (spec/areas/settlement.md#schemes-and-ledgers).
    pub(super) fn load_schemes(&mut self, store: &Store) -> Result<(), String> {
        for (id, config) in store.settlement_schemes().map_err(|e| e.to_string())? {
            if super::schemes::DISABLED.contains(&id.as_str()) {
                self.report(SettlementEvent::Disabled { scheme: id });
                continue;
            }
            let ledger = super::SCHEMES
                .iter()
                .find(|info| info.id == id.as_str())
                .map(|info| info.ledger);
            if ledger.is_some_and(|ledger| ledger.realm() != super::ledger_realm(self.realm)) {
                self.report(SettlementEvent::OtherRealm { scheme: id });
                continue;
            }
            let checkpoint = self.checkpoints.get(&id).cloned();
            self.activate(id, config, checkpoint, false)
                .map_err(|e| format!("settlement scheme {id}: {e}"))?;
        }
        Ok(())
    }

    /// Activates a settlement scheme. Its ID must be one the node knows,
    /// where the original's RPC took any text (RT-23).
    ///
    /// # Errors
    ///
    /// The scheme is unknown, disabled, refuses its config, or is active.
    pub fn add_scheme(&mut self, id: &str, config: String) -> Result<LedgerView, EngineError> {
        if super::schemes::DISABLED.contains(&id) {
            return Err(EngineError::Precondition(
                "the scheme is disabled (spec/decisions/issued-assets.md)".into(),
            ));
        }
        let id =
            Name::new(id).map_err(|_| EngineError::Invalid("unknown settlement scheme".into()))?;
        if self.schemes.contains_key(&id) {
            return Err(EngineError::Precondition("the scheme is active".into()));
        }
        let checkpoint = self.checkpoints.get(&id).cloned();
        let ledger = self.activate(id, config, checkpoint, true)?;
        // Settlements left pending when it was last removed (ACC-16).
        self.reconcile_settlements(Some(id));
        self.reconcile_owner_payments();
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Starts a scheme: makes it and the external asset it settles through,
    /// opens its ledger's accounts, books the difference to its checkpoint
    /// (ACC-6), and makes sure the receiver has a ledger.
    fn activate(
        &mut self,
        id: Name,
        config: String,
        checkpoint: Option<Checkpoint>,
        new: bool,
    ) -> Result<LedgerId, EngineError> {
        let (mut scheme, asset) = self.make_scheme(id, &config, checkpoint.as_ref())?;
        let over_hub = scheme.commitments().is_some();
        let converts = scheme.converts();
        let ledger = scheme.ledger();
        if let Err(error) = self.open_ledger(id, scheme.as_ref(), checkpoint.as_ref()) {
            if let Some(asset) = asset {
                self.release_asset(asset);
            }
            return Err(error);
        }
        if new {
            self.batch.schemes_added.push((id, config));
        }
        self.schemes.insert(
            id,
            ActiveScheme {
                scheme,
                asset,
                version: 0,
                failed: None,
                over_hub,
                // A guard paused before a restart is told again.
                paused: None,
                converts,
            },
        );
        if let Some(asset) = asset {
            // The scheme tells its asset where it is, before the asset reads
            // its ledger for it.
            self.tell_scheme(id, asset);
            self.carry_out(asset);
            self.connect_for_scheme(asset);
        }
        self.checkpoint(id);
        self.update_peer_schemes();
        self.add_receiver(ledger)?;
        self.add_payer(ledger)?;
        Ok(ledger)
    }

    /// Opens the accounts of `scheme`'s ledger and books the difference
    /// between its balance and its checkpoint (ACC-6): only a ledger of the
    /// node's realm, and one no other scheme serves. The realm gate: a node
    /// runs only its realm's ledgers, so test and live nodes never share a
    /// scheme, and a ledger of real money runs only where the live realm
    /// may, which it may not but by the dev switch
    /// (spec/decisions/mainnet-schemes.md).
    fn open_ledger(
        &mut self,
        id: Name,
        scheme: &dyn SettlementScheme,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<(), EngineError> {
        let ledger = scheme.ledger();
        if ledger.realm() != super::ledger_realm(self.realm) {
            return Err(EngineError::Precondition(
                "the scheme is of another realm than the node's".into(),
            ));
        }
        if ledger.realm() == Realm::Live && !self.live.allowed() {
            return Err(EngineError::Precondition(
                "real money is not supported yet (spec/decisions/real-money.md)".into(),
            ));
        }
        if self
            .schemes
            .values()
            .any(|active| active.scheme.ledger() == ledger)
        {
            return Err(EngineError::Precondition(
                "another scheme serves that ledger".into(),
            ));
        }
        for name in COMMON_ACCOUNTS {
            self.ledger
                .open(AccountPath::new(ledger, name))
                .map_err(|e| EngineError::Internal(e.to_string()))?;
        }
        let settlement = self.account(ledger, AccountName::AssetsSettlement)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        let suspense = self.account(ledger, AccountName::EquitySuspense)?;
        let booking = match activation(checkpoint.map(|c| c.balance), scheme.balance()) {
            Activation::InStep => None,
            Activation::FirstDeposit(amount) => {
                Some((settlement, owner, amount, Reference::Deposit))
            }
            Activation::Gain(amount) => {
                Some((settlement, suspense, amount, Reference::Reconciliation))
            }
            Activation::Loss(amount) => {
                Some((suspense, settlement, amount, Reference::Reconciliation))
            }
        };
        if let Some((debit, credit, amount, reference)) = booking {
            self.post(debit, credit, amount, reference)?;
            self.report(SettlementEvent::BalanceBooked {
                scheme: id,
                reference,
                amount,
            });
        }
        Ok(())
    }

    /// Deactivates a settlement scheme, and only it: the original's removal
    /// cut its list after the first scheme (ACC-10).
    ///
    /// # Errors
    ///
    /// The scheme is not active, or tokens use its ledger.
    pub fn remove_scheme(&mut self, id: &str) -> Result<(), EngineError> {
        let id =
            Name::new(id).map_err(|_| EngineError::NotFound("the scheme is not active".into()))?;
        let ledger = match self.schemes.get(&id) {
            Some(active) => active.scheme.ledger(),
            None => return Err(EngineError::NotFound("the scheme is not active".into())),
        };
        if let Some(blocker) = self.removal_blockers(id, ledger).first() {
            return Err(EngineError::Precondition(blocker.message().into()));
        }
        if let Some(asset) = self.schemes.remove(&id).and_then(|active| active.asset) {
            if let Some(active) = self.assets.get_mut(&asset) {
                active.asset.detach(id, Detach::Removed);
            }
            self.release_asset(asset);
        }
        self.update_peer_schemes();
        self.batch.schemes_added.retain(|(added, _)| *added != id);
        self.batch.schemes_removed.push(id);
        let now = self.now_ms();
        if let Some(receiver) = self.receivers.remove(&ledger) {
            self.connector.remove_endpoint(now, receiver.endpoint);
        }
        if let Some(payer) = self.spending.payers.remove(&ledger) {
            self.connector.remove_endpoint(now, payer.endpoint);
        }
        self.expire(now);
        Ok(())
    }

    /// Adds test money through the stub.
    ///
    /// # Errors
    ///
    /// The stub is not active, or stopped, or the books cannot take it.
    pub fn stub_deposit(&mut self, amount: u64) -> Result<LedgerView, EngineError> {
        let stub = dassie_ledger::name!("stub");
        let inactive = || EngineError::Precondition("the stub scheme is not active".into());
        let ledger = self
            .schemes
            .get(&stub)
            .map(|active| active.scheme.ledger())
            .ok_or_else(inactive)?;
        if self.schemes.get(&stub).is_some_and(|a| a.failed.is_some()) {
            return Err(EngineError::Precondition(
                "the stub scheme has stopped; restart the node".into(),
            ));
        }
        // The books must take it before the stub's balance moves: they
        // could not follow it otherwise.
        let deposit = TransferSpec {
            debit: self.account(ledger, AccountName::AssetsSettlement)?,
            credit: self.account(ledger, AccountName::EquityOwner)?,
            amount,
            pending: false,
            reference: Reference::Deposit,
        };
        self.ledger
            .validate(&deposit)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let active = self.schemes.get_mut(&stub).ok_or_else(inactive)?;
        let mut effects = Vec::new();
        active
            .scheme
            .deposit(amount, &mut effects)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.apply_effects(stub, ledger, &effects)?;
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Asks the faucet of a scheme's test ledger for money. What it sends is
    /// booked as a deposit once the ledger shows it.
    ///
    /// # Errors
    ///
    /// The scheme is not active, or stopped, has no faucet, or asked it
    /// too recently.
    pub fn ask_faucet(&mut self, id: &str) -> Result<LedgerView, EngineError> {
        let inactive = || EngineError::NotFound("the scheme is not active".into());
        let id = Name::new(id).map_err(|_| inactive())?;
        let active = self.schemes.get(&id).ok_or_else(inactive)?;
        if active.failed.is_some() {
            return Err(EngineError::Precondition(
                "the scheme has stopped; restart the node".into(),
            ));
        }
        let ledger = active.scheme.ledger();
        self.asset_faucet(id).map_err(|error| match error {
            SchemeError::Unsupported => {
                EngineError::Precondition("the ledger has no faucet".into())
            }
            SchemeError::Unavailable => EngineError::Exhausted(
                "the faucet was asked less than a minute ago, or the ledger cannot be reached"
                    .into(),
            ),
            other => EngineError::Internal(other.to_string()),
        })?;
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Books what a scheme reports, and checkpoints its balance in the
    /// same batch (ACC-6). All or none: the books check every transfer the
    /// effects make, in order, before they book any, and if they refuse
    /// one, none of the effects is carried out. A refusal is a bug, which
    /// stops the scheme, so its checkpoint stays with what the books hold
    /// (spec/areas/settlement.md#the-scheme-interface).
    pub(super) fn apply_effects(
        &mut self,
        scheme: Name,
        ledger: LedgerId,
        effects: &[Effect],
    ) -> Result<(), EngineError> {
        let bookings = match self.check_effects(scheme, ledger, effects) {
            Ok(bookings) => bookings,
            Err(error) => {
                self.stop_scheme(scheme, error.clone());
                return Err(error);
            }
        };
        assert_eq!(bookings.len(), effects.len(), "one booking per effect");
        for (effect, booking) in effects.iter().zip(bookings) {
            match effect {
                Effect::Deposit { .. } | Effect::Withdrawal { .. } | Effect::Fee { .. } => {
                    self.book(booking);
                }
                Effect::Reconcile {
                    amount,
                    gain,
                    cause,
                } => {
                    self.report(SettlementEvent::Reconciled {
                        scheme,
                        amount: *amount,
                        gain: *gain,
                        cause: *cause,
                    });
                    self.book(booking);
                }
                Effect::IncomingSettlement { peer, amount, id } => {
                    assert!(booking.is_some(), "a settlement books a transfer");
                    if let Some(spec) = booking {
                        self.settlement_in(scheme, *peer, *amount, *id, spec);
                    }
                }
                Effect::PeerLoss {
                    peer,
                    amount,
                    cause,
                    ..
                } => {
                    self.report(SettlementEvent::PeerLoss {
                        scheme,
                        peer: *peer,
                        amount: *amount,
                        cause: *cause,
                    });
                    self.book(booking);
                    if *cause == LossCause::Cheated {
                        self.end_for_cheating(*peer);
                    }
                }
                Effect::Finalize { settlement: id } => self.settlement_done(scheme, *id),
                Effect::OwnerPaid { id, applied } => self.owner_paid(scheme, *id, *applied),
                Effect::Cancel { settlement: id } => self.settlement_cancelled(scheme, *id),
                Effect::SendMessage { peer, message } => {
                    let peer = dassie_identity::NodeId::parse(peer.as_str());
                    let id = dassie_peer::wire::SchemeId::parse(scheme.as_str());
                    if let (Ok(peer), Some(id)) = (peer, id)
                        && let Err(error) =
                            self.peer
                                .send_scheme_message(self.now_ms, &peer, id, message)
                    {
                        self.report(SettlementEvent::MessageNotSent { peer, error });
                    }
                }
            }
        }
        self.take_journal();
        self.checkpoint_if_changed(scheme);
        Ok(())
    }

    /// The transfer each of a scheme's effects books, if any, once the
    /// books have checked them all: each as they would find the accounts
    /// with the ones before it booked. Posting or voiding one of our
    /// settlements (`Finalize`, `Cancel`) or the hold of an owner's payment
    /// (`OwnerPaid`) is not checked: it raises no
    /// account's total and tightens no limit, so it cannot make a later
    /// transfer fail, and it is never refused itself.
    fn check_effects(
        &self,
        scheme: Name,
        ledger: LedgerId,
        effects: &[Effect],
    ) -> Result<Vec<Option<TransferSpec>>, EngineError> {
        let accounts = [
            self.account(ledger, AccountName::AssetsSettlement)?,
            self.account(ledger, AccountName::EquityOwner)?,
            self.account(ledger, AccountName::EquitySuspense)?,
        ];
        let mut bookings = Vec::with_capacity(effects.len());
        for effect in effects {
            let booking = self.effect_transfer(scheme, ledger, accounts, effect)?;
            bookings.push(
                booking.map(|(debit, credit, amount, reference)| TransferSpec {
                    debit,
                    credit,
                    amount,
                    pending: false,
                    reference,
                }),
            );
        }
        // Nothing to book for nothing: the journal holds no empty transfers.
        let specs: Vec<TransferSpec> = bookings
            .iter()
            .flatten()
            .filter(|spec| spec.amount > 0)
            .copied()
            .collect();
        self.ledger
            .validate_posted(&specs)
            .map_err(|(_, error)| EngineError::Invalid(error.to_string()))?;
        Ok(bookings)
    }

    /// What one of a scheme's effects books, if anything: debit, credit,
    /// amount and reference. `accounts` are the ledger's settlement, owner
    /// and suspense accounts.
    fn effect_transfer(
        &self,
        scheme: Name,
        ledger: LedgerId,
        accounts: [AccountId; 3],
        effect: &Effect,
    ) -> Result<Option<(AccountId, AccountId, u64, Reference)>, EngineError> {
        let [settlement, owner, suspense] = accounts;
        let transfer = match effect {
            Effect::Deposit { amount } => (settlement, owner, *amount, Reference::Deposit),
            Effect::Withdrawal { amount } => (owner, settlement, *amount, Reference::Withdrawal),
            Effect::Reconcile {
                amount, gain: true, ..
            } => (settlement, suspense, *amount, Reference::Reconciliation),
            Effect::Reconcile {
                amount,
                gain: false,
                ..
            } => (suspense, settlement, *amount, Reference::Reconciliation),
            Effect::Fee { amount, id } => {
                let fees = self.account(ledger, AccountName::ExpensesFees)?;
                (
                    fees,
                    settlement,
                    *amount,
                    Reference::Fee { scheme, id: *id },
                )
            }
            // A peer with no accounts on the ledger cannot have settled
            // with us: what it sent is a deposit. One without an ID
            // gets the next transfer's as it is booked, which the check
            // does not look at: a settlement is not a report either way.
            Effect::IncomingSettlement { peer, amount, id } => {
                let path = AccountPath::new(ledger, AccountName::AssetsInterledger(*peer));
                match self.ledger.find(&path) {
                    Some(interledger) => (
                        settlement,
                        interledger,
                        *amount,
                        Reference::Settlement {
                            scheme,
                            id: id.unwrap_or(scheme),
                        },
                    ),
                    None => (settlement, owner, *amount, Reference::Deposit),
                }
            }
            Effect::PeerLoss {
                peer,
                amount,
                cause,
                id,
            } => self.loss_transfer(scheme, ledger, settlement, *peer, *cause, *amount, *id)?,
            Effect::Finalize { .. }
            | Effect::Cancel { .. }
            | Effect::OwnerPaid { .. }
            | Effect::SendMessage { .. } => {
                return Ok(None);
            }
        };
        Ok(Some(transfer))
    }

    /// What a peering over a hub cost beyond its hold: Dr `expenses/losses`,
    /// Cr the account the money left, as its cause says.
    #[allow(clippy::too_many_arguments)]
    fn loss_transfer(
        &self,
        scheme: Name,
        ledger: LedgerId,
        settlement: AccountId,
        peer: Name,
        cause: LossCause,
        amount: u64,
        id: Name,
    ) -> Result<(AccountId, AccountId, u64, Reference), EngineError> {
        let losses = self.account(ledger, AccountName::ExpensesLosses)?;
        let from = match cause {
            LossCause::ClaimFee => settlement,
            LossCause::PaidLate | LossCause::Cheated => {
                self.account(ledger, AccountName::AssetsInterledger(peer))?
            }
        };
        Ok((losses, from, amount, Reference::Loss { scheme, id }))
    }

    /// Books a transfer of a batch the books checked whole.
    // Panics only on a broken invariant: what the check took, the books
    // take, with the journal drained before each.
    #[allow(clippy::panic)]
    fn book(&mut self, spec: Option<TransferSpec>) {
        let Some(spec) = spec.filter(|spec| spec.amount > 0) else {
            return;
        };
        self.take_journal();
        if let Err(error) = self.ledger.create(self.now_ms(), &spec) {
            panic!("the books refused a transfer they had checked: {error}");
        }
        self.take_journal();
    }

    /// A peer's settlement, as the scheme saw it arrive: credits the peer's
    /// account, keyed by the scheme's ID for it where it has one, or, from
    /// a node with no accounts on the ledger, the owner's
    /// ([`check_effects`](Self::check_effects)).
    fn settlement_in(
        &mut self,
        scheme: Name,
        peer: Name,
        amount: u64,
        id: Option<Name>,
        spec: TransferSpec,
    ) {
        if spec.reference == Reference::Deposit {
            self.report(SettlementEvent::DepositFromStranger { peer, amount });
            self.book(Some(spec));
            return;
        }
        let reference = match id {
            Some(id) => Reference::Settlement { scheme, id },
            None => self.incoming_reference(scheme),
        };
        self.book(Some(TransferSpec { reference, ..spec }));
        // It answered, and our books show it: it may be asked again at the
        // next check (spec/decisions/own-fulfills.md). Not on its message
        // alone, which on a ledger like the XRP Ledger books nothing: asked
        // again before our books moved, an honest peer would pay twice. And a
        // peer that settles is not stuck: the grace before it is withdrawn
        // for want of room starts again
        // (spec/decisions/peering-lifecycle.md). A peer whose peering we end
        // collects what it said was on its way
        // (spec/decisions/ending-a-peering.md).
        if let Ok(node) = dassie_identity::NodeId::parse(peer.as_str()) {
            self.settle.asked.remove(&node);
            self.peer
                .room(self.now_ms, &node, dassie_peer::Room::Enough);
            // Counted toward what it said was on its way only on the
            // peering's scheme, whose units the answer is in.
            if self
                .peer
                .peering_of(&node)
                .is_some_and(|peering| peering.scheme.as_str() == scheme.as_str())
            {
                self.peer.settlement_booked(self.now_ms, &node, amount);
            }
            self.report(SettlementEvent::SettlementBooked { from: node, amount });
        }
    }

    fn post(
        &mut self,
        debit: AccountId,
        credit: AccountId,
        amount: u64,
        reference: Reference,
    ) -> Result<(), EngineError> {
        // Nothing to book; the journal holds no empty transfers.
        if amount == 0 {
            return Ok(());
        }
        let now = self.now_ms();
        let spec = TransferSpec {
            debit,
            credit,
            amount,
            pending: false,
            reference,
        };
        self.ledger
            .create(now, &spec)
            .map(|_| ())
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.take_journal();
        Ok(())
    }

    pub(super) fn account(
        &self,
        ledger: LedgerId,
        name: AccountName,
    ) -> Result<AccountId, EngineError> {
        self.ledger
            .find(&AccountPath::new(ledger, name))
            .ok_or_else(|| EngineError::Internal(format!("{ledger}:{name} is missing")))
    }

    /// Gives a ledger its STREAM receiver, a local endpoint paying into the
    /// owner's pool.
    fn add_receiver(&mut self, ledger: LedgerId) -> Result<(), EngineError> {
        if self.receivers.contains_key(&ledger) {
            return Ok(());
        }
        let endpoint = self.free_local();
        let own = self.connector.own_address().to_string();
        let receiver = Receiver::new(&self.key, &own, ledger, endpoint)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        self.connector
            .add_endpoint(endpoint, &receiver_address(&own, ledger), owner, ledger)
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        self.receivers.insert(ledger, receiver);
        Ok(())
    }

    /// The next local endpoint, skipping any in use: a number comes back
    /// only after 65 536 activations, long after anything for it is gone.
    pub(super) fn free_local(&mut self) -> dassie_connector::EndpointId {
        let taken = |n: u16| {
            let endpoint = dassie_connector::EndpointId::Local(n);
            self.receivers
                .values()
                .any(|receiver| receiver.endpoint == endpoint)
                || self
                    .spending
                    .payers
                    .values()
                    .any(|payer| payer.endpoint == endpoint)
        };
        let mut free = self.next_local;
        for _ in 0..=super::MAX_LOCAL_ENDPOINTS {
            if !taken(free) {
                break;
            }
            free = free.wrapping_add(1);
        }
        assert!(!taken(free), "more local endpoints than there may be");
        self.next_local = free.wrapping_add(1);
        dassie_connector::EndpointId::Local(free)
    }

    /// The active schemes' IDs.
    pub fn active_schemes(&self) -> Vec<String> {
        self.schemes
            .keys()
            .map(|id| id.as_str().to_string())
            .collect()
    }

    /// The ledgers of active schemes.
    pub fn ledger_views(&self) -> Vec<LedgerView> {
        let mut ledgers: Vec<LedgerId> = self
            .schemes
            .values()
            .map(|active| active.scheme.ledger())
            .collect();
        ledgers.sort();
        ledgers
            .into_iter()
            .filter_map(|ledger| self.ledger_view(ledger))
            .collect()
    }

    /// What keeps scheme `id`, serving `ledger`, from being removed: the
    /// one list both the wallet and [`remove_scheme`](Self::remove_scheme)
    /// go by.
    fn removal_blockers(&self, id: Name, ledger: LedgerId) -> Vec<RemovalBlocker> {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let tokens = self
            .tokens
            .values()
            .filter(|token| token.row.revoked_at_ms.is_none() && token.row.ledger == ledger)
            .count();
        let peers = self
            .peer
            .peers()
            .filter(|(_, peering)| peering.scheme.as_str() == id.as_str())
            .count();
        [
            (tokens, RemovalBlocker::BtpTokens(count(tokens))),
            (peers, RemovalBlocker::Peers(count(peers))),
            (
                self.unfinished_on(ledger),
                RemovalBlocker::OutgoingPayments(count(self.unfinished_on(ledger))),
            ),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(_, blocker)| blocker)
        .collect()
    }

    fn ledger_view(&self, ledger: LedgerId) -> Option<LedgerView> {
        let (id, active) = self
            .schemes
            .iter()
            .find(|(_, active)| active.scheme.ledger() == ledger)?;
        let owner = self.account(ledger, AccountName::EquityOwner).ok()?;
        let group = self.ledger.group_balance(owner)?;
        let asset = active.asset.and_then(|asset| self.assets.get(&asset));
        Some(LedgerView {
            id: ledger,
            scheme: id.as_str().to_string(),
            owner_balance: group.net_credit(),
            owner_pending: group.debits_pending,
            on_ledger_balance: active.scheme.balance(),
            reserve: active.scheme.reserve(),
            deposit_address: asset.and_then(|asset| asset.asset.address()),
            issuer: active.scheme.issuer(),
            faucet: asset.is_some_and(|asset| asset.asset.has_faucet()),
            status: match active.failed {
                Some(_) => LedgerStatus::Failed(
                    "the books could not take what the ledger said; restart the node",
                ),
                None => self.scheme_status(active),
            },
            peg: active.scheme.peg(),
            removal_blockers: self.removal_blockers(*id, ledger),
        })
    }
}
