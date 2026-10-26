//! Dassie's internal ledger: double-entry accounts, pending and posted
//! transfers, and the journal that makes them durable.
//!
//! - [`LedgerId`] names the ledgers the node knows and their assets.
//! - [`AccountPath`] names an account on a ledger; its kind decides its
//!   limit and its group.
//! - [`Ledger`] holds the accounts and the pending transfers, checks limits,
//!   and queues a [`JournalEntry`] for everything the database must keep.
//!
//! The design is inspired by TigerBeetle (spec/areas/ledger.md), with these
//! rules:
//!
//! - transfers have IDs, and refusals are values, never assertions on input;
//! - linked transfers are created all or none;
//! - opening an account that exists returns it unchanged;
//! - the owner's accounts on a ledger form a group whose total carries the
//!   spending limit, so the owner cannot overspend, while each
//!   spending endpoint has its own sub-account for attribution
//!   (spec/areas/ledger.md#the-owners-accounts).
//!
//! Nothing here performs IO or reads a clock: callers pass `now_ms` in and
//! persist what [`Ledger::poll_journal`] hands them, before they act on the
//! result (spec/decisions/tigerstyle.md).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

mod account;
mod ledger;
mod ledger_id;
mod name;
mod path;
mod reference;

pub use account::{Account, AccountId, Balance, Limit};
pub use ledger::{
    Config, JournalEntry, Ledger, LedgerError, RestoreError, Stats, StoredAccount, Transfer,
    TransferError, TransferId, TransferSpec, TransferState,
};
pub use ledger_id::{Asset, LedgerId, Realm};
pub use name::{InvalidName, Name};
pub use path::{AccountName, AccountPath, InvalidPath, OwnerSub};
pub use reference::{Reference, TraceId};

/// The largest total any account counter may reach: SQLite stores the
/// cached totals as signed 64-bit integers. At scale 9 this is about 9.2e9
/// units of the asset over an account's lifetime.
pub const MAX_TOTAL: u64 = i64::MAX.unsigned_abs();
