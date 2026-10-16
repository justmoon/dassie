//! A database in memory that applies journal entries as the node's store
//! does, so tests can check that the journal alone rebuilds the ledger.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use dassie_ledger::{
    AccountId, AccountName, AccountPath, Config, JournalEntry, Ledger, LedgerId, StoredAccount,
    Transfer, TransferId, TransferState,
};

#[derive(Default, Debug)]
pub struct Mirror {
    pub accounts: Vec<StoredAccount>,
    pub durable_pending: BTreeMap<TransferId, Transfer>,
    /// Every transfer row, by ID: its final or current state.
    pub rows: BTreeMap<TransferId, (Transfer, TransferState)>,
    pub next_transfer: u64,
}

impl Mirror {
    /// Persists everything the ledger has queued.
    pub fn drain(&mut self, ledger: &mut Ledger) {
        while let Some(entry) = ledger.poll_journal() {
            self.apply(entry);
        }
        self.next_transfer = self.next_transfer.max(ledger.next_transfer_id().0);
    }

    pub fn apply(&mut self, entry: JournalEntry) {
        match entry {
            JournalEntry::AccountOpened { id, path } => {
                assert_eq!(id.0 as usize, self.accounts.len(), "accounts open in order");
                self.accounts.push(StoredAccount {
                    id,
                    path,
                    debits_posted: 0,
                    credits_posted: 0,
                });
            }
            JournalEntry::Transfer {
                transfer, state, ..
            } => {
                let previous = self.rows.insert(transfer.id, (transfer, state));
                match state {
                    TransferState::Pending => {
                        assert!(previous.is_none(), "a pending row is new");
                        self.durable_pending.insert(transfer.id, transfer);
                    }
                    TransferState::Posted => {
                        if let Some((_, previous)) = previous {
                            assert_eq!(previous, TransferState::Pending);
                        }
                        self.durable_pending.remove(&transfer.id);
                        let debit = &mut self.accounts[transfer.debit.0 as usize];
                        debit.debits_posted += transfer.amount;
                        let credit = &mut self.accounts[transfer.credit.0 as usize];
                        credit.credits_posted += transfer.amount;
                    }
                    TransferState::Voided => {
                        assert_eq!(previous.map(|(_, s)| s), Some(TransferState::Pending));
                        self.durable_pending.remove(&transfer.id);
                    }
                }
            }
        }
    }

    pub fn restore(&self, config: Config) -> Ledger {
        Ledger::restore(
            config,
            self.accounts.iter().copied(),
            self.durable_pending.values().copied(),
            TransferId(self.next_transfer),
        )
        .expect("the journal rebuilds the ledger")
    }
}

pub fn path(name: AccountName) -> AccountPath {
    AccountPath::new(LedgerId::StubUsd, name)
}

pub fn open(ledger: &mut Ledger, name: AccountName) -> AccountId {
    ledger.open(path(name)).unwrap()
}

/// Posted net credit of an account.
pub fn net(ledger: &Ledger, id: AccountId) -> i128 {
    ledger.account(id).unwrap().balance().net_credit()
}
