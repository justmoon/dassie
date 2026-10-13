//! Settlement schemes and the ledgers they serve.

use dassie_ledger::{
    AccountId, AccountName, AccountPath, LedgerId, Name, Realm, Reference, TransferSpec,
};
use dassie_settlement::{Activation, Effect, SettlementScheme, activation};

use super::packets::{RECEIVER, RECEIVER_SEGMENT, Receiver};
use super::state::Core;
use super::{Credentials, EngineError};
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
}

/// The accounts every active ledger has (spec 5 §2.6).
const COMMON_ACCOUNTS: [AccountName; 7] = [
    AccountName::AssetsSettlement,
    AccountName::EquityOwner,
    AccountName::EquitySuspense,
    AccountName::RevenueFx,
    AccountName::RevenueFees,
    AccountName::ExpensesFx,
    AccountName::ExpensesFees,
];

impl Core {
    /// Activates the stored schemes, booking what changed while the node was
    /// down.
    pub(super) fn load_schemes(&mut self, store: &Store) -> Result<(), String> {
        let checkpoints = store.checkpoints().map_err(|e| e.to_string())?;
        for (id, config) in store.settlement_schemes().map_err(|e| e.to_string())? {
            let checkpoint = checkpoints
                .iter()
                .find(|(scheme, _)| *scheme == id)
                .map(|&(_, balance)| balance);
            self.activate(id, config, checkpoint, false)
                .map_err(|e| format!("settlement scheme {id}: {e}"))?;
        }
        Ok(())
    }

    pub(super) fn add_scheme(
        &mut self,
        id: &str,
        config: String,
    ) -> Result<LedgerView, EngineError> {
        let id =
            Name::new(id).map_err(|_| EngineError::Invalid("unknown settlement scheme".into()))?;
        if self.schemes.contains_key(&id) {
            return Err(EngineError::Precondition("the scheme is active".into()));
        }
        let checkpoint = self
            .node
            .store()
            .checkpoints()?
            .into_iter()
            .find(|(scheme, _)| *scheme == id)
            .map(|(_, balance)| balance);
        let ledger = self.activate(id, config, checkpoint, true)?;
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
        if new {
            self.batch.schemes_added.push((id, config));
        }
        self.schemes.insert(id, ActiveScheme { scheme });
        self.ensure_receiver()?;
        Ok(ledger)
    }

    pub(super) fn remove_scheme(&mut self, id: &str) -> Result<(), EngineError> {
        let id =
            Name::new(id).map_err(|_| EngineError::NotFound("the scheme is not active".into()))?;
        let ledger = match self.schemes.get(&id) {
            Some(active) => active.scheme.ledger(),
            None => return Err(EngineError::NotFound("the scheme is not active".into())),
        };
        if self
            .tokens
            .values()
            .any(|token| token.row.revoked_at_ms.is_none() && token.row.ledger == ledger)
        {
            return Err(EngineError::Precondition(
                "BTP tokens use its ledger".into(),
            ));
        }
        self.schemes.remove(&id);
        self.batch.schemes_removed.push(id);
        if self
            .receiver
            .as_ref()
            .is_some_and(|receiver| receiver.ledger == ledger)
        {
            let now = self.now_ms();
            self.connector.remove_endpoint(now, RECEIVER);
            self.receiver = None;
            self.expire(now);
            self.ensure_receiver()?;
        }
        Ok(())
    }

    pub(super) fn stub_deposit(&mut self, amount: u64) -> Result<LedgerView, EngineError> {
        let stub = Name::new("stub").map_err(|_| EngineError::Internal("name".into()))?;
        let Some(active) = self.schemes.get_mut(&stub) else {
            return Err(EngineError::Precondition(
                "the stub scheme is not active".into(),
            ));
        };
        let mut effects = Vec::new();
        active
            .scheme
            .deposit(amount, &mut effects)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let ledger = active.scheme.ledger();
        let balance = active.scheme.balance();
        self.apply_effects(ledger, &effects)?;
        self.batch.checkpoints.push((stub, balance));
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Books what a scheme reports. Settlements with peers come in M4.
    fn apply_effects(&mut self, ledger: LedgerId, effects: &[Effect]) -> Result<(), EngineError> {
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
                other => {
                    tracing::warn!(target: target::SETTLEMENT, ?other, "settlement effect without peers");
                }
            }
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

    /// Gives the receiver the first active ledger, if it has none.
    fn ensure_receiver(&mut self) -> Result<(), EngineError> {
        if self.receiver.is_some() {
            return Ok(());
        }
        let Some(ledger) = self
            .schemes
            .values()
            .map(|active| active.scheme.ledger())
            .min()
        else {
            return Ok(());
        };
        let receiver = Receiver::new(&self.key, self.connector.own_address(), ledger)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        let address = format!("{}.{RECEIVER_SEGMENT}", self.connector.own_address());
        self.connector
            .add_endpoint(RECEIVER, &address, owner, ledger)
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        self.receiver = Some(receiver);
        Ok(())
    }

    pub(super) fn stream_credentials(&mut self, tag: &str) -> Result<Credentials, EngineError> {
        if tag.len() > dassie_stream::token::MAX_CONNECTION_TAG_LENGTH {
            return Err(EngineError::Invalid("the tag is too long".into()));
        }
        let Self { node, receiver, .. } = self;
        let Some(receiver) = receiver.as_ref() else {
            return Err(EngineError::Precondition(
                "no settlement scheme is active".into(),
            ));
        };
        let credentials = node.with_rng(|rng| {
            receiver
                .server
                .generate_credentials(rng, tag.as_bytes(), None)
        });
        Ok(Credentials {
            destination: credentials.destination,
            shared_secret: credentials.shared_secret,
            asset: receiver.ledger.asset(),
        })
    }

    pub(super) fn active_schemes(&self) -> Vec<String> {
        self.schemes
            .keys()
            .map(|id| id.as_str().to_string())
            .collect()
    }

    pub(super) fn ledger_views(&self) -> Vec<LedgerView> {
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
        })
    }
}
