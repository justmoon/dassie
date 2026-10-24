//! A lying server (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
//! a connection can be made dishonest with any one of the lies a server
//! can tell a node that checks the validators, each from the honest
//! answer, so it is as close to the truth as it can be and still a lie.
//! A lying server holds no validator's key, nor the secret of a
//! simulation's validations: it can withhold and replay validations, and
//! make up anything else, never forge a validation.
//!
//! Each lie is counted as it is told, so a run can show that every kind
//! was reached.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use serde_json::Value;

use super::{ConnectionId, FakeLedger};

/// What a lying server says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lie {
    /// A transaction no ledger holds: `account_tx` gives a made-up Payment
    /// of test XRP to the account asked about, and `tx_proof` proves it,
    /// or a transaction of the account's that never applied, in a header
    /// made up with it.
    Transaction,
    /// A fake balance: each account root and trust line `state_proof`
    /// gives holds more than it does, with its honest branch.
    AlteredObject,
    /// A false absence: each object `state_proof` holds is said to be
    /// absent, with its honest branch.
    FalseAbsence,
    /// The branch of another key: each object `state_proof` holds is
    /// answered with another object and that one's branch.
    OtherBranch,
    /// An old anchor with its old validations: the validated ledger is one
    /// so old its validations are stale.
    OldAnchor,
    /// An old anchor after a newer one: the validated ledger is one a few
    /// ledgers back, its validations still fresh.
    OlderAnchor,
    /// Forged validations: under the keys of the validators that signed,
    /// signed by a key that is none of theirs.
    ForgedValidations,
    /// Replayed validations: those of the ledger before.
    ReplayedValidations,
    /// Withheld validations: one validator's only, short of a quorum.
    WithheldValidations,
    /// A header that hashes wrong: its close time a second later.
    WrongHeader,
    /// A skip list of another chain: the hashes it holds are none of this
    /// chain's, with its honest branch.
    OtherChainSkipList,
    /// History left out at the newest end: `account_tx` without the
    /// account's newest transaction, and no transactions on the stream.
    HistoryNewestLeftOut,
    /// History left out in the middle: `account_tx` without one of the
    /// account's transactions between the oldest and the newest.
    HistoryMiddleLeftOut,
    /// A hint naming an old ledger for a recent transaction: `account_tx`,
    /// `tx` and the stream name a ledger before the history, and
    /// `tx_proof` answers `lgrNotFound`.
    OldLedgerHint,
    /// Refusals within the history: the proof methods answer that the
    /// server is too busy.
    Refusal,
    /// A reset with a made-up header: the validated ledger is a lower one
    /// that closed later, of a chain holding no account but genesis.
    MadeUpReset,
    /// Not a lie, a server that serves no proofs: a stock rippled, which
    /// knows none of the proof methods.
    NoProofs,
}

impl Lie {
    /// Every kind.
    pub const ALL: [Self; 17] = [
        Self::Transaction,
        Self::AlteredObject,
        Self::FalseAbsence,
        Self::OtherBranch,
        Self::OldAnchor,
        Self::OlderAnchor,
        Self::ForgedValidations,
        Self::ReplayedValidations,
        Self::WithheldValidations,
        Self::WrongHeader,
        Self::OtherChainSkipList,
        Self::HistoryNewestLeftOut,
        Self::HistoryMiddleLeftOut,
        Self::OldLedgerHint,
        Self::Refusal,
        Self::MadeUpReset,
        Self::NoProofs,
    ];

    /// Its name, as `fake_ledger` takes it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Transaction => "transaction",
            Self::AlteredObject => "altered-object",
            Self::FalseAbsence => "false-absence",
            Self::OtherBranch => "other-branch",
            Self::OldAnchor => "old-anchor",
            Self::OlderAnchor => "older-anchor",
            Self::ForgedValidations => "forged-validations",
            Self::ReplayedValidations => "replayed-validations",
            Self::WithheldValidations => "withheld-validations",
            Self::WrongHeader => "wrong-header",
            Self::OtherChainSkipList => "other-chain-skip-list",
            Self::HistoryNewestLeftOut => "history-newest-left-out",
            Self::HistoryMiddleLeftOut => "history-middle-left-out",
            Self::OldLedgerHint => "old-ledger-hint",
            Self::Refusal => "refusal",
            Self::MadeUpReset => "made-up-reset",
            Self::NoProofs => "no-proofs",
        }
    }

    /// The lie named `name`.
    #[must_use]
    pub fn by_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|lie| lie.name() == name)
    }
}

/// Who lies how, and what was told.
#[derive(Clone, Debug, Default)]
pub(super) struct Lies {
    /// Each lying connection's lie.
    by_connection: BTreeMap<ConnectionId, Lie>,
    /// The lie of every connection not named.
    every: Option<Lie>,
    /// How often each lie was told.
    told: BTreeMap<Lie, u64>,
}

impl FakeLedger {
    /// Makes `connection` tell `lie` from now on, or be honest again.
    pub fn set_lie(&mut self, connection: ConnectionId, lie: Option<Lie>) {
        match lie {
            Some(lie) => self.lies.by_connection.insert(connection, lie),
            None => self.lies.by_connection.remove(&connection),
        };
    }

    /// Makes every connection, open or to come, tell `lie` from now on, or
    /// be honest again.
    pub fn set_lie_for_all(&mut self, lie: Option<Lie>) {
        self.lies.by_connection.clear();
        self.lies.every = lie;
    }

    /// How often `lie` was told.
    #[must_use]
    pub fn lies_told(&self, lie: Lie) -> u64 {
        self.lies.told.get(&lie).copied().unwrap_or(0)
    }

    /// The lie `connection` tells, if it lies.
    pub(super) fn lie(&self, connection: ConnectionId) -> Option<Lie> {
        (self.lies.by_connection.get(&connection).copied()).or(self.lies.every)
    }

    /// `connection` lies `lie`, if it is the one it tells.
    pub(super) fn lies_with(&self, connection: ConnectionId, lie: Lie) -> bool {
        self.lie(connection) == Some(lie)
    }

    /// `lie` was told.
    pub(super) fn told(&mut self, lie: Lie) {
        let count = self.lies.told.entry(lie).or_insert(0);
        *count = count.saturating_add(1);
    }

    /// A connection went away: it tells nothing more.
    pub(super) fn forget_liar(&mut self, connection: ConnectionId) {
        self.lies.by_connection.remove(&connection);
    }

    /// A ledger before the history, which a lie names for a recent
    /// transaction.
    fn old_ledger(&self) -> u32 {
        self.history_from.saturating_sub(1).max(1)
    }

    /// What the account's stream carries of a transaction to
    /// `connection`, if anything.
    pub(super) fn streamed(
        &mut self,
        connection: ConnectionId,
        message: &Value,
    ) -> Option<Vec<u8>> {
        let lie = self.lie(connection);
        match lie {
            Some(Lie::HistoryNewestLeftOut) => {
                self.told(Lie::HistoryNewestLeftOut);
                return None;
            }
            Some(Lie::OldLedgerHint) => {
                let mut message = message.clone();
                set(&mut message, "ledger_index", self.old_ledger().into());
                self.told(Lie::OldLedgerHint);
                return serde_json::to_vec(&message).ok();
            }
            _ => {}
        }
        serde_json::to_vec(message).ok()
    }

    /// `server_state`, as `connection` has it: the validated ledger an
    /// older one, or a made-up reset's.
    pub(super) fn server_state(&self, connection: ConnectionId) -> Value {
        let mut state = self.honest_server_state();
        let header = match self.validated_lie(connection) {
            Some((_, Some(index))) => self.header(index),
            Some((_, None)) => Some(self.made_up_chain().0),
            None => None,
        };
        let validated =
            (state.get_mut("state")).and_then(|state| state.get_mut("validated_ledger"));
        if let (Some(header), Some(validated)) = (header, validated) {
            set(validated, "seq", header.sequence.into());
            set(validated, "close_time", header.close_time.into());
        }
        state
    }

    /// `account_tx`, as `connection` has it.
    pub(super) fn account_tx(
        &mut self,
        connection: ConnectionId,
        request: &Value,
    ) -> Result<Value, Value> {
        let mut result = self.honest_account_tx(request)?;
        let Some(lie) = self.lie(connection) else {
            return Ok(result);
        };
        let last_page = result.get("marker").is_none();
        let made_up = match lie {
            Lie::Transaction if last_page => Self::account_of(request)
                .ok()
                .and_then(|id| self.made_up_entry(id)),
            _ => None,
        };
        let old = self.old_ledger();
        let Some(transactions) = (result.get_mut("transactions")).and_then(Value::as_array_mut)
        else {
            return Ok(result);
        };
        let length = transactions.len();
        let told = match lie {
            Lie::HistoryNewestLeftOut if last_page && length > 0 => {
                transactions.pop();
                true
            }
            Lie::HistoryMiddleLeftOut if length >= 3 => {
                transactions.remove(length / 2);
                true
            }
            Lie::OldLedgerHint if length > 0 => {
                for entry in transactions.iter_mut() {
                    if let Some(tx) = entry.get_mut("tx") {
                        set(tx, "ledger_index", old.into());
                    }
                }
                true
            }
            Lie::Transaction => made_up.is_some_and(|entry| {
                transactions.push(entry);
                true
            }),
            _ => false,
        };
        if told {
            self.told(lie);
        }
        Ok(result)
    }

    /// `tx`, as `connection` has it.
    pub(super) fn tx(&mut self, connection: ConnectionId, request: &Value) -> Result<Value, Value> {
        let answer = self.honest_tx(request);
        match (self.lie(connection), answer) {
            (Some(Lie::OldLedgerHint), Ok(mut found)) => {
                set(&mut found, "ledger_index", self.old_ledger().into());
                self.told(Lie::OldLedgerHint);
                Ok(found)
            }
            (Some(Lie::Transaction), Err(not_found)) => {
                let asked =
                    (request.get("transaction").and_then(Value::as_str)).and_then(crate::from_hex);
                let made_up = (self.accounts.keys())
                    .filter_map(|account| self.made_up_entry(*account))
                    .find(|entry| {
                        let hash = entry.get("tx").and_then(|tx| tx.get("hash"));
                        let hash = hash.and_then(Value::as_str).and_then(crate::from_hex);
                        hash.is_some() && hash == asked
                    });
                let Some(entry) = made_up else {
                    return Err(not_found);
                };
                let mut found = entry.get("tx").cloned().unwrap_or(Value::Null);
                set(
                    &mut found,
                    "meta",
                    entry.get("meta").cloned().unwrap_or(Value::Null),
                );
                set(&mut found, "validated", true.into());
                self.told(Lie::Transaction);
                Ok(found)
            }
            (_, answer) => answer,
        }
    }
}

/// Sets `key` of the object `value` to `field`.
fn set(value: &mut Value, key: &str, field: Value) {
    if let Some(object) = value.as_object_mut() {
        object.insert(key.into(), field);
    }
}
