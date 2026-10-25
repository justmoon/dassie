//! The ledger: accounts, pending transfers and the journal outbox.

mod invariants;
mod restore;
mod transfers;

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::fmt;

pub use restore::{RestoreError, StoredAccount};

use crate::account::{Account, AccountId, Balance};
use crate::ledger_id::LedgerId;
use crate::name::Name;
use crate::path::{AccountName, AccountPath};
use crate::reference::Reference;

/// The ledger's bounds (spec/decisions/tigerstyle.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Most accounts.
    pub max_accounts: u32,
    /// Most pending transfers.
    pub max_pending: u32,
    /// Most journal entries waiting for the caller to persist them.
    pub max_journal: u32,
}

impl Config {
    /// Room for the owner's accounts on every ledger, a few hundred BTP
    /// tokens and peers, and the packets a node has in flight.
    pub const DEFAULT: Self = Self {
        max_accounts: 1024,
        max_pending: 8192,
        max_journal: 1024,
    };

    /// Checks the bounds make sense.
    ///
    /// # Panics
    ///
    /// If one is zero.
    pub fn validate(&self) {
        assert!(self.max_accounts > 0, "a ledger needs accounts");
        assert!(self.max_pending > 0, "a ledger needs pending transfers");
        assert!(self.max_journal > 1, "a journal entry and its account fit");
    }
}

/// Identifies a transfer. Assigned in order, never reused, and resumed
/// after the largest persisted one on restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransferId(pub u64);

/// A transfer to create.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferSpec {
    /// The account debited.
    pub debit: AccountId,
    /// The account credited.
    pub credit: AccountId,
    /// How much. Never zero.
    pub amount: u64,
    /// Pending, to be posted or voided later, rather than posted now.
    pub pending: bool,
    /// What it is for.
    pub reference: Reference,
}

/// A transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transfer {
    /// Its ID.
    pub id: TransferId,
    /// The account debited.
    pub debit: AccountId,
    /// The account credited.
    pub credit: AccountId,
    /// How much.
    pub amount: u64,
    /// What it is for.
    pub reference: Reference,
    /// When it was created.
    pub created_at_ms: u64,
}

/// Where a transfer is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferState {
    /// Created, holding its amount, not yet final.
    Pending,
    /// Final: the amount moved.
    Posted,
    /// Final: the amount did not move.
    Voided,
}

/// Something the database must keep. The caller persists the entries in
/// order, all of one step in one transaction, before acting on the step's
/// outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalEntry {
    /// An account was opened. Its counters start at zero.
    AccountOpened {
        /// Its ID.
        id: AccountId,
        /// Its path.
        path: AccountPath,
    },
    /// A transfer was created or changed state. A posted transfer moves its
    /// amount into the accounts' posted totals.
    Transfer {
        /// The transfer.
        transfer: Transfer,
        /// Its new state.
        state: TransferState,
        /// When it got there.
        at_ms: u64,
    },
}

/// Why a transfer was not created. Values, never panics: the input may
/// come from a peer (ACC-8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// An account does not exist.
    UnknownAccount,
    /// Debit and credit are the same account.
    SameAccount,
    /// The accounts are on different ledgers.
    DifferentLedgers,
    /// The amount is zero.
    ZeroAmount,
    /// The debit would break a limit.
    ExceedsDebits,
    /// The credit would break a limit.
    ExceedsCredits,
    /// A counter would pass [`crate::MAX_TOTAL`].
    Overflow,
    /// Too many transfers are pending.
    PendingFull,
    /// The journal is full: the caller has not persisted it.
    JournalFull,
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownAccount => "unknown account",
            Self::SameAccount => "debit and credit are the same account",
            Self::DifferentLedgers => "the accounts are on different ledgers",
            Self::ZeroAmount => "zero amount",
            Self::ExceedsDebits => "exceeds the debit limit",
            Self::ExceedsCredits => "exceeds the credit limit",
            Self::Overflow => "the account's total would overflow",
            Self::PendingFull => "too many pending transfers",
            Self::JournalFull => "the journal is full",
        })
    }
}

/// Why an account could not be opened, or a transfer resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// The account table is full.
    AccountsFull,
    /// The journal is full: the caller has not persisted it.
    JournalFull,
    /// No pending transfer has that ID.
    UnknownTransfer,
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AccountsFull => "the account table is full",
            Self::JournalFull => "the journal is full",
            Self::UnknownTransfer => "no such pending transfer",
        })
    }
}

/// Counters for instrumentation and simulation checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Transfers created, pending or posted.
    pub created: u64,
    /// Transfers posted, at creation or later.
    pub posted: u64,
    /// Pending transfers voided.
    pub voided: u64,
    /// Transfers refused for a limit.
    pub refused: u64,
}

/// The ledger.
#[derive(Debug)]
pub struct Ledger {
    config: Config,
    accounts: Vec<Account>,
    by_path: BTreeMap<AccountPath, AccountId>,
    pending: BTreeMap<TransferId, Transfer>,
    next_transfer: u64,
    journal: VecDeque<JournalEntry>,
    stats: Stats,
}

impl Ledger {
    /// An empty ledger.
    ///
    /// # Panics
    ///
    /// If the config does not validate.
    #[must_use]
    pub fn new(config: Config) -> Self {
        config.validate();
        Self {
            config,
            accounts: Vec::with_capacity(usize_of(config.max_accounts)),
            by_path: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_transfer: 0,
            journal: VecDeque::with_capacity(usize_of(config.max_journal)),
            stats: Stats::default(),
        }
    }

    /// The account at `path`, opening it, and its group root first, if
    /// needed. Opening an account that exists returns it unchanged (ACC-4).
    ///
    /// # Errors
    ///
    /// If the table or the journal has no room.
    pub fn open(&mut self, path: AccountPath) -> Result<AccountId, LedgerError> {
        if let Some(&id) = self.by_path.get(&path) {
            return Ok(id);
        }
        let root = path.group_root().filter(|root| *root != path);
        let missing_root = root.filter(|root| !self.by_path.contains_key(root));
        let needed = 1 + usize::from(missing_root.is_some());
        if self.accounts.len() + needed > usize_of(self.config.max_accounts) {
            return Err(LedgerError::AccountsFull);
        }
        if self.journal.len() + needed > usize_of(self.config.max_journal) {
            return Err(LedgerError::JournalFull);
        }
        if let Some(root) = missing_root {
            self.insert_account(root, None);
        }
        let group = root.and_then(|root| self.by_path.get(&root).copied());
        Ok(self.insert_account(path, group))
    }

    /// Adds an account the caller made room for. A root is its own group.
    fn insert_account(&mut self, path: AccountPath, group: Option<AccountId>) -> AccountId {
        let id = account_id(self.accounts.len());
        let group = group.or_else(|| path.group_root().map(|_| id));
        self.accounts.push(Account::new(path, group));
        self.by_path.insert(path, id);
        self.journal
            .push_back(JournalEntry::AccountOpened { id, path });
        id
    }

    /// The accounts a peer needs: what it owes us, and the credit we extend
    /// to it. When they are new, `trust` is posted as that credit; when they
    /// exist, nothing is posted again (ACC-5). Returns the interledger
    /// account and whether the accounts were new.
    ///
    /// # Errors
    ///
    /// If there is no room.
    ///
    /// # Panics
    ///
    /// If `trust` passes [`crate::MAX_TOTAL`]: it is configuration.
    pub fn open_peer(
        &mut self,
        now_ms: u64,
        ledger: LedgerId,
        node: Name,
        trust: u64,
    ) -> Result<(AccountId, bool), LedgerError> {
        assert!(
            trust <= crate::MAX_TOTAL,
            "trust beyond any account's total"
        );
        let interledger = AccountPath::new(ledger, AccountName::AssetsInterledger(node));
        let contra = AccountPath::new(ledger, AccountName::ContraTrust(node));
        if let Some(&id) = self.by_path.get(&interledger) {
            return Ok((id, false));
        }
        if self.accounts.len() + 2 > usize_of(self.config.max_accounts) {
            return Err(LedgerError::AccountsFull);
        }
        if self.journal.len() + 3 > usize_of(self.config.max_journal) {
            return Err(LedgerError::JournalFull);
        }
        let interledger = self.open(interledger)?;
        let contra = self.open(contra)?;
        if trust > 0 {
            let trusted = self.create(
                now_ms,
                &TransferSpec {
                    debit: contra,
                    credit: interledger,
                    amount: trust,
                    pending: false,
                    reference: Reference::Trust,
                },
            );
            // Fresh accounts with room checked: nothing can refuse it.
            assert!(trusted.is_ok(), "trust to new accounts failed: {trusted:?}");
        }
        Ok((interledger, true))
    }

    /// Takes back the accounts of `node` on `ledger` that
    /// [`open_peer`](Self::open_peer) opened last, with the credit it
    /// posted to them, as if they had never been opened: for a caller that
    /// opened them for a transfer the ledger then refused
    /// (spec/decisions/lazy-downlink-accounts.md). Nothing may have been
    /// booked on them since, nor any account opened after them, and what
    /// they wrote must still wait in the journal.
    ///
    /// # Panics
    ///
    /// If they are not the accounts opened last, as `open_peer` left them.
    pub fn unopen_peer(&mut self, ledger: LedgerId, node: Name) {
        let interledger = AccountPath::new(ledger, AccountName::AssetsInterledger(node));
        let contra = AccountPath::new(ledger, AccountName::ContraTrust(node));
        let count = self.accounts.len();
        assert!(count >= 2, "no accounts to take back");
        let (interledger_id, contra_id) = (account_id(count - 2), account_id(count - 1));
        assert!(
            self.by_path.get(&interledger) == Some(&interledger_id)
                && self.by_path.get(&contra) == Some(&contra_id),
            "the peer's accounts are the last opened"
        );
        let untouched = |account: &Account| {
            account.balance.debits_pending == 0 && account.balance.credits_pending == 0
        };
        assert!(
            self.accounts
                .get(count - 2..)
                .is_some_and(|accounts| accounts.iter().all(untouched)),
            "nothing is pending on the accounts taken back"
        );
        // The credit open_peer posted, if any: nothing else was booked.
        if let Some(JournalEntry::Transfer { transfer, .. }) = self.journal.back().copied() {
            assert!(
                transfer.debit == contra_id
                    && transfer.credit == interledger_id
                    && transfer.reference == Reference::Trust,
                "only the opening credit was booked on the accounts taken back"
            );
            self.journal.pop_back();
            self.stats.created -= 1;
            self.stats.posted -= 1;
        }
        for id in [contra_id, interledger_id] {
            let entry = self.journal.pop_back();
            assert!(
                matches!(entry, Some(JournalEntry::AccountOpened { id: opened, .. }) if opened == id),
                "the accounts' opening still waits in the journal"
            );
        }
        self.accounts.truncate(count - 2);
        self.by_path.remove(&interledger);
        self.by_path.remove(&contra);
    }

    /// The account `id`.
    #[must_use]
    pub fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.get(usize_of(id.0))
    }

    /// The account at `path`, if open.
    #[must_use]
    pub fn find(&self, path: &AccountPath) -> Option<AccountId> {
        self.by_path.get(path).copied()
    }

    /// Every account, in ID order.
    pub fn accounts(&self) -> impl Iterator<Item = (AccountId, &Account)> {
        self.accounts
            .iter()
            .enumerate()
            .map(|(index, account)| (account_id(index), account))
    }

    /// The total of the group whose root is `root`, without the pending
    /// transfers between its members, which cannot change it.
    #[must_use]
    pub fn group_balance(&self, root: AccountId) -> Option<Balance> {
        self.account(root)
            .filter(|account| account.group == Some(root))
            .map(Account::group_view)
    }

    /// Counts a pending transfer between members of one group in, or out,
    /// of that group's internal pending amount.
    fn internal_pending(&mut self, transfer: &Transfer, add: bool) {
        let debit = self
            .account(transfer.debit)
            .and_then(|account| account.group);
        let credit = self
            .account(transfer.credit)
            .and_then(|account| account.group);
        let Some(root) = debit.filter(|_| debit == credit) else {
            return;
        };
        if let Some(root) = self.accounts.get_mut(usize_of(root.0)) {
            let amount = u128::from(transfer.amount);
            if add {
                root.group_internal_pending += amount;
            } else {
                root.group_internal_pending -= amount;
            }
        }
    }

    /// The pending transfer `id`.
    #[must_use]
    pub fn pending(&self, id: TransferId) -> Option<&Transfer> {
        self.pending.get(&id)
    }

    /// Every pending transfer, in ID order.
    pub fn pending_transfers(&self) -> impl Iterator<Item = &Transfer> {
        self.pending.values()
    }

    /// The next journal entry to persist.
    pub fn poll_journal(&mut self) -> Option<JournalEntry> {
        self.journal.pop_front()
    }

    /// How many journal entries wait.
    #[must_use]
    pub fn journal_len(&self) -> usize {
        self.journal.len()
    }

    /// The ID the next transfer gets.
    #[must_use]
    pub const fn next_transfer_id(&self) -> TransferId {
        TransferId(self.next_transfer)
    }

    /// Counters.
    #[must_use]
    pub const fn stats(&self) -> Stats {
        self.stats
    }

    /// The bounds.
    #[must_use]
    pub const fn config(&self) -> Config {
        self.config
    }
}

/// Bounds are `u32`s; every target has at least 32-bit `usize`.
fn usize_of(value: u32) -> usize {
    const { assert!(usize::BITS >= u32::BITS) };
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// The ID of the account at `index`: below `max_accounts`, a `u32`.
fn account_id(index: usize) -> AccountId {
    let id = u32::try_from(index);
    assert!(id.is_ok(), "more accounts than a u32 counts");
    AccountId(id.unwrap_or_default())
}
