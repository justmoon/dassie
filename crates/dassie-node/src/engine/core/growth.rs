//! Credit that grows with what a peering earned us
//! (spec/decisions/earned-trust.md). At every settlement check, the fees our
//! connector earned on packets from or to a peer, booked on that peer's
//! ledger, raise the credit we extend it by
//! [`policy::GROWTH_PPM`] of them, up to [`policy::credit_cap`].
//!
//! What a peer earned is read from the journal, and counted nowhere else:
//! the trust account holds the credit it led to. At a start, reading
//! resumes after the last trust transfer. So a crash forgets the fees not
//! read yet, those of the check it cut short, and never credits a fee
//! twice. A node's first start with growth reads from its last trust
//! transfer too, so what peers earned since then counts.

use std::collections::{BTreeMap, BTreeSet};

use dassie_ledger::{
    AccountId, AccountName, AccountPath, Ledger, LedgerId, Name, Reference, TransferError,
    TransferSpec,
};
use dassie_settlement::policy;

use super::Core;
use super::events::SettlementEvent;
use crate::engine::EngineError;

/// Fees read per ledger at a check; the rest wait for the next
/// (spec/decisions/tigerstyle.md). At a check every 4 s, over a thousand
/// fee-paying packets a second, so reading keeps up with a busy connector
/// rather than fall behind.
const FEES_PER_CHECK: u32 = 4_096;

/// Millionths, the unit of [`policy::GROWTH_PPM`].
const PPM: u128 = 1_000_000;

/// Where reading the fees stands.
#[derive(Debug)]
pub(super) struct Growth {
    /// The journal sequence after which the fees of each ledger are read
    /// next.
    cursors: BTreeMap<LedgerId, u64>,
    /// Where every ledger starts, from the last trust transfer: read at the
    /// first check.
    start: Option<u64>,
    /// What growth fell short of a whole unit, by peer account, in
    /// millionths of a unit. At most one entry per peer account.
    carry: BTreeMap<AccountId, u128>,
}

impl Growth {
    pub(super) const fn new() -> Self {
        Self {
            cursors: BTreeMap::new(),
            start: None,
            carry: BTreeMap::new(),
        }
    }
}

impl Core {
    /// Reads the fees earned since the last check, and raises the credit
    /// of each peer they were earned through.
    pub(super) fn grow_credit(&mut self) {
        let start = match self.settle.growth.start {
            Some(start) => start,
            None => match self.store.last_trust_sequence() {
                Ok(last) => *self.settle.growth.start.insert(last.unwrap_or(0)),
                Err(error) => {
                    self.report(SettlementEvent::EarningsNotRead {
                        error: error.into(),
                    });
                    return;
                }
            },
        };
        let ledgers: BTreeSet<LedgerId> = self
            .schemes
            .values()
            .map(|active| active.scheme.ledger())
            .collect();
        let mut earned: BTreeMap<AccountId, u128> = BTreeMap::new();
        for ledger in ledgers {
            let path = AccountPath::new(ledger, AccountName::RevenueFees);
            let Some(fees) = self.ledger.find(&path) else {
                continue;
            };
            let after = self
                .settle
                .growth
                .cursors
                .get(&ledger)
                .copied()
                .unwrap_or(start);
            let rows = match self.store.fees_after(fees, after, FEES_PER_CHECK) {
                Ok(rows) => rows,
                Err(error) => {
                    self.report(SettlementEvent::EarningsNotRead {
                        error: error.into(),
                    });
                    continue;
                }
            };
            for row in &rows {
                // The peer it came from, and the one it went to on this
                // ledger: the FX account, across ledgers, is no peer's.
                for account in [Some(row.from), row.onward].into_iter().flatten() {
                    if peer_of(&self.ledger, account).is_some() {
                        *earned.entry(account).or_default() += u128::from(row.amount);
                    }
                }
            }
            if let Some(last) = rows.last() {
                self.settle.growth.cursors.insert(ledger, last.sequence);
            }
        }
        for (account, fees) in earned {
            self.raise_for(account, fees);
        }
    }

    /// Raises the credit of the peer whose account is `account` by its share
    /// of `fees`, up to the cap.
    fn raise_for(&mut self, account: AccountId, fees: u128) {
        let Some((ledger, name)) = peer_of(&self.ledger, account) else {
            return;
        };
        let path = AccountPath::new(ledger, AccountName::ContraTrust(name));
        let Some(credit) = self
            .ledger
            .find(&path)
            .and_then(|contra| self.ledger.account(contra))
            .map(|contra| policy::trust(contra.balance()))
        else {
            return;
        };
        let cap = self.settle.credit_cap;
        if credit >= cap {
            self.settle.growth.carry.remove(&account);
            return;
        }
        let growth = self.settle.growth.carry.remove(&account).unwrap_or(0)
            + fees * u128::from(policy::GROWTH_PPM);
        let (whole, rest) = (growth / PPM, growth % PPM);
        if rest > 0 {
            self.settle.growth.carry.insert(account, rest);
        }
        let raised = policy::raised(credit, whole, cap);
        if raised == credit {
            return;
        }
        match raise_trust(&mut self.ledger, self.now_ms, ledger, name, raised - credit) {
            Ok(()) => self.report(SettlementEvent::CreditRaised {
                peer: name,
                credit: raised,
            }),
            Err(error) => self.report(SettlementEvent::EarningsNotRead {
                error: EngineError::Internal(format!("raising {name}'s credit: {error}")),
            }),
        }
    }
}

/// The ledger and the node of a peer's `assets/interledger` account, if
/// `account` is one.
fn peer_of(ledger: &Ledger, account: AccountId) -> Option<(LedgerId, Name)> {
    let path = ledger.account(account)?.path();
    match path.name {
        AccountName::AssetsInterledger(name) => Some((path.ledger, name)),
        _ => None,
    }
}

/// Raises the credit we extend to `node` on `ledger` by `amount`: a trust
/// transfer, as when it was first extended.
pub(super) fn raise_trust(
    ledger: &mut Ledger,
    now_ms: u64,
    on: LedgerId,
    node: Name,
    amount: u64,
) -> Result<(), TransferError> {
    let find = |name| ledger.find(&AccountPath::new(on, name));
    let (Some(contra), Some(interledger)) = (
        find(AccountName::ContraTrust(node)),
        find(AccountName::AssetsInterledger(node)),
    ) else {
        return Err(TransferError::UnknownAccount);
    };
    ledger
        .create(
            now_ms,
            &TransferSpec {
                debit: contra,
                credit: interledger,
                amount,
                pending: false,
                reference: Reference::Trust,
            },
        )
        .map(|_| ())
}
