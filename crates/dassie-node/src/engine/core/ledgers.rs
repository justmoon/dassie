//! Settlement schemes and the ledgers they serve.

use dassie_ledger::{
    AccountId, AccountName, AccountPath, LedgerId, Name, Realm, Reference, TransferSpec,
};
use dassie_settlement::{Activation, Effect, SettlementScheme, activation};

use super::Core;
use super::EngineError;
use super::packets::{Receiver, receiver_address};
use crate::log::target;
use crate::store::Store;

/// An active scheme.
pub(super) struct ActiveScheme {
    pub scheme: Box<dyn SettlementScheme>,
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
    /// The scheme's balance on the underlying ledger.
    pub on_ledger_balance: u64,
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
    /// Peerings that ended still have to be settled with it.
    FinalSettlements(u32),
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
            Self::FinalSettlements(_) => "a peering that ended still has to be settled with it",
            Self::OutgoingPayments(_) => "outgoing payments from its ledger have not finished",
        }
    }
}

/// The accounts every active ledger has (spec 5 §2.6).
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

impl Core {
    /// Activates the stored schemes, booking what changed while the node was
    /// down.
    pub(super) fn load_schemes(&mut self, store: &Store) -> Result<(), String> {
        for (id, config) in store.settlement_schemes().map_err(|e| e.to_string())? {
            let checkpoint = self.checkpoints.get(&id).copied();
            self.activate(id, config, checkpoint, false)
                .map_err(|e| format!("settlement scheme {id}: {e}"))?;
        }
        Ok(())
    }

    /// Activates a settlement scheme.
    ///
    /// # Errors
    ///
    /// The scheme is unknown, refuses its config, or is active.
    pub fn add_scheme(&mut self, id: &str, config: String) -> Result<LedgerView, EngineError> {
        let id =
            Name::new(id).map_err(|_| EngineError::Invalid("unknown settlement scheme".into()))?;
        if self.schemes.contains_key(&id) {
            return Err(EngineError::Precondition("the scheme is active".into()));
        }
        let checkpoint = self.checkpoints.get(&id).copied();
        let ledger = self.activate(id, config, checkpoint, true)?;
        // Settlements left pending when it was last removed (ACC-16).
        self.reconcile_settlements(Some(id));
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Starts a scheme: opens its ledger's accounts, books the difference
    /// to its checkpoint (ACC-6), and makes sure the receiver has a ledger.
    fn activate(
        &mut self,
        id: Name,
        config: String,
        checkpoint: Option<u64>,
        new: bool,
    ) -> Result<LedgerId, EngineError> {
        let scheme = dassie_settlement::create(id.as_str(), &config, checkpoint)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let ledger = scheme.ledger();
        if ledger.realm() != Realm::Test {
            return Err(EngineError::Precondition(
                "real money is not supported yet (D8)".into(),
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
        let balance = scheme.balance();
        let settlement = self.account(ledger, AccountName::AssetsSettlement)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        let suspense = self.account(ledger, AccountName::EquitySuspense)?;
        let booking = match activation(checkpoint, balance) {
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
            tracing::info!(target: target::SETTLEMENT, scheme = %id, ?reference, amount, "booked the scheme's balance");
        }
        self.batch.checkpoints.push((id, balance));
        self.checkpoints.insert(id, balance);
        if new {
            self.batch.schemes_added.push((id, config));
        }
        self.schemes.insert(id, ActiveScheme { scheme });
        self.update_peer_schemes();
        self.add_receiver(ledger)?;
        self.add_payer(ledger)?;
        Ok(ledger)
    }

    /// Deactivates a settlement scheme.
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
        self.schemes.remove(&id);
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
    /// The stub is not active, or the books cannot take it.
    pub fn stub_deposit(&mut self, amount: u64) -> Result<LedgerView, EngineError> {
        let stub = Name::new("stub").map_err(|_| EngineError::Internal("name".into()))?;
        let inactive = || EngineError::Precondition("the stub scheme is not active".into());
        let ledger = self
            .schemes
            .get(&stub)
            .map(|active| active.scheme.ledger())
            .ok_or_else(inactive)?;
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

    /// Books what a scheme reports, and checkpoints its balance in the
    /// same batch (ACC-6).
    pub(super) fn apply_effects(
        &mut self,
        scheme: Name,
        ledger: LedgerId,
        effects: &[Effect],
    ) -> Result<(), EngineError> {
        let settlement = self.account(ledger, AccountName::AssetsSettlement)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        for effect in effects {
            match effect {
                Effect::Deposit { amount } => {
                    self.post(settlement, owner, *amount, Reference::Deposit)?;
                }
                Effect::Withdrawal { amount } => {
                    self.post(owner, settlement, *amount, Reference::Withdrawal)?;
                }
                Effect::IncomingSettlement { peer, amount } => {
                    let interledger =
                        self.account(ledger, AccountName::AssetsInterledger(*peer))?;
                    let reference = self.incoming_reference(scheme);
                    self.post(settlement, interledger, *amount, reference)?;
                    if let Ok(node) = dassie_identity::NodeId::parse(peer.as_str()) {
                        self.grace_settled(node);
                    }
                }
                Effect::Finalize { settlement: id } => self.settlement_done(scheme, *id),
                Effect::Cancel { settlement: id } => self.settlement_cancelled(scheme, *id),
                Effect::SendMessage { peer, message } => {
                    let peer = dassie_identity::NodeId::parse(peer.as_str());
                    let id = dassie_peer::wire::SchemeId::parse(scheme.as_str());
                    if let (Ok(peer), Some(id)) = (peer, id)
                        && let Err(error) =
                            self.peer
                                .send_scheme_message(self.now_ms, &peer, id, message)
                    {
                        tracing::debug!(target: target::SETTLEMENT, %peer, ?error, "a scheme message was not sent");
                    }
                }
            }
        }
        self.take_journal();
        if let Some(active) = self.schemes.get(&scheme) {
            let balance = active.scheme.balance();
            self.batch.checkpoints.push((scheme, balance));
            self.checkpoints.insert(scheme, balance);
        }
        Ok(())
    }

    fn post(
        &mut self,
        debit: AccountId,
        credit: AccountId,
        amount: u64,
        reference: Reference,
    ) -> Result<(), EngineError> {
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
        let finals = self
            .peer
            .channels()
            .filter(|(_, channel)| {
                channel
                    .final_due
                    .as_ref()
                    .is_some_and(|due| due.scheme.as_str() == id.as_str())
            })
            .count();
        [
            (tokens, RemovalBlocker::BtpTokens(count(tokens))),
            (peers, RemovalBlocker::Peers(count(peers))),
            (finals, RemovalBlocker::FinalSettlements(count(finals))),
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
        Some(LedgerView {
            id: ledger,
            scheme: id.as_str().to_string(),
            owner_balance: group.net_credit(),
            owner_pending: group.debits_pending,
            on_ledger_balance: active.scheme.balance(),
            removal_blockers: self.removal_blockers(*id, ledger),
        })
    }
}
