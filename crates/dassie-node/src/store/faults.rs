//! A commit that fails, the one fault a simulation injects at the store
//! (spec/decisions/random-seed-simulations.md). The store commits with
//! `synchronous = FULL`, so a commit that returned is on disk, and a torn
//! write is SQLite's to handle: what is left to go wrong is a commit that
//! fails, as on a full disk or an I/O error. SQLite then rolls the
//! transaction back and reports an error, and the database stays at the
//! commit before.
//!
//! The fault can be aimed at the commits that matter: one that books a
//! packet's Fulfill, or one that holds a settlement. Only the engine's
//! commits ([`Store::commit_ledger`]) are aimed at; nothing else writes
//! what moves money.

use std::sync::{Arc, Mutex, PoisonError};

use dassie_ledger::{JournalEntry, Reference};
use rusqlite::Connection;

use super::{LedgerBatch, Store};

/// Which of the engine's commits fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aim {
    /// Any.
    Any,
    /// One that books a packet's transfer: a Fulfill taken or sent.
    Packet,
    /// One that holds a settlement's transfer: ours prepared, executed or
    /// cancelled, or one a peer sent us booked.
    Settlement,
}

/// What a batch held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Holds {
    /// A packet's transfer.
    pub packet: bool,
    /// A settlement's transfer.
    pub settlement: bool,
}

impl Holds {
    fn of(batch: &LedgerBatch) -> Self {
        let mut holds = Self::default();
        for entry in &batch.journal {
            if let JournalEntry::Transfer { transfer, .. } = entry {
                match transfer.reference {
                    Reference::Packet(_) => holds.packet = true,
                    Reference::Settlement { .. } => holds.settlement = true,
                    _ => {}
                }
            }
        }
        holds
    }

    const fn aimed(self, aim: Aim) -> bool {
        match aim {
            Aim::Any => true,
            Aim::Packet => self.packet,
            Aim::Settlement => self.settlement,
        }
    }
}

/// The commit made to fail, as its maker sees it: what that commit held,
/// once it failed.
#[derive(Clone, Debug, Default)]
pub struct FailedCommit(Arc<Mutex<Option<Holds>>>);

impl FailedCommit {
    /// What the failed commit held, once it failed.
    #[must_use]
    pub fn held(&self) -> Option<Holds> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A commit the store is to fail.
#[derive(Debug)]
pub(super) struct Failing {
    aim: Aim,
    /// Commits it aims at that still go through first.
    skip: u32,
    failed: FailedCommit,
}

impl Store {
    /// Makes the commit after the next `skip` that `aim` aims at fail.
    #[must_use]
    pub fn fail_commit(&self, aim: Aim, skip: u32) -> FailedCommit {
        self.let_commits_through();
        let failed = FailedCommit::default();
        *self.failing() = Some(Failing {
            aim,
            skip,
            failed: failed.clone(),
        });
        failed
    }

    /// Lets every commit through again, and leaves the connection without
    /// the commit hook a failed commit left on it.
    pub fn let_commits_through(&self) {
        *self.failing() = None;
        let cleared = self.lock().commit_hook(None::<fn() -> bool>).is_ok();
        assert!(cleared, "the store's own connection takes a commit hook");
    }

    fn failing(&self) -> std::sync::MutexGuard<'_, Option<Failing>> {
        self.failing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Before the engine's commit of `batch`: if it is the one to fail,
    /// arms SQLite's commit hook to turn its COMMIT into a ROLLBACK.
    pub(super) fn fail_if_aimed(&self, connection: &Connection, batch: &LedgerBatch) {
        let mut failing = self.failing();
        let holds = Holds::of(batch);
        let Some(aimed) = failing.as_mut().filter(|f| holds.aimed(f.aim)) else {
            return;
        };
        if aimed.skip > 0 {
            aimed.skip -= 1;
            return;
        }
        let Some(Failing { failed, .. }) = failing.take() else {
            return;
        };
        *failed.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(holds);
        let mut once = true;
        let hooked = connection
            .commit_hook(Some(move || std::mem::take(&mut once)))
            .is_ok();
        assert!(hooked, "the store's own connection takes a commit hook");
    }
}
