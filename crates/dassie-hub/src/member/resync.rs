//! What the scheme books when its account read its holds anew
//! (spec/areas/hubs.md#when-the-history-is-gone): the validators no longer
//! keep what touched the account since its cursor, so what happened in
//! between is known only by its difference.
//!
//! - **A hold toward us from a peer** can only grow by its payer's funds, and
//!   a take moves what it holds to what was taken: what it holds and what was
//!   taken from it grew by the payer's settlements in between, booked as
//!   theirs.
//! - **Our hold toward a peer**: what the payee took in between beyond what
//!   we owed it and it had not taken is what the hub paid with claims we
//!   never read. It is a loss on the peer ([`LossCause::PaidLate`]), and
//!   counts in T from now on, since the hub overrules our books.
//! - **Our settlement on its way** whose sequence was used: applied if the
//!   hold shows its funds, cancelled otherwise; its fee, and anything else
//!   unexplained, the account's state reconciles through suspense.
//! - **Waiting packets** are decided by the block read anew as by any final
//!   block: without a fulfillment once it is stamped at or after their
//!   commitment's expiry, since a claim the account never read is not one
//!   it can fulfill upstream with. What the hub paid for them is in the loss
//!   above.
//!
//! What a take moved in between that the view does not show (`claim`s above
//! the redeemed W) makes these differences err on the side of booking less
//! as a settlement or a loss, and more through suspense.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_ledger::Name;
use dassie_settlement::{Effect, LossCause, ReconcileCause};

use super::account::{HubAccount, Purpose};
use super::mirror::HoldMirror;
use super::payer::Loss;
use super::scheme::HubScheme;
use super::{UNITS_PER_DROP, units};
use crate::digest::Digest;
use crate::operation::MemberOperation;
use crate::promise::HoldNumber;
use crate::rules::Amount;
use dassie_settlement::OwnerPaymentState;

/// What a hold was funded with, as far as its mirror says: what it holds
/// and what was taken from it.
fn funded(hold: &HoldMirror) -> Amount {
    hold.amount.saturating_add(hold.taken())
}

/// The ID of what a resync books for `hold` at `height`, for `kind`.
fn resync_id(kind: u8, hold: HoldNumber, height: u64) -> Name {
    let mut bytes = [0u8; 17];
    let parts = core::iter::once(kind)
        .chain(hold.0.to_be_bytes())
        .chain(height.to_be_bytes());
    for (slot, byte) in bytes.iter_mut().zip(parts) {
        *slot = byte;
    }
    Name::hex(&bytes, false)
}

impl HubScheme {
    /// Books what the account read anew as of the block at `height`.
    pub(super) fn resynced(
        &mut self,
        account: &mut HubAccount,
        height: u64,
        before: &[(HoldNumber, HoldMirror)],
        unread: Option<&(Purpose, MemberOperation)>,
        effects: &mut Vec<Effect>,
    ) {
        let before: BTreeMap<HoldNumber, &HoldMirror> = before
            .iter()
            .map(|(number, hold)| (*number, hold))
            .collect();
        let funds = match unread {
            Some((Purpose::Settle(settlement), operation)) => {
                self.settled_unread(account, &before, *settlement, operation, effects)
            }
            Some((purpose @ Purpose::Claim(_), _)) => {
                self.dropped(*purpose, effects);
                None
            }
            // Whether it applied, only the account's counters say, and
            // they book it, its ledger fee with it.
            Some((Purpose::Withdraw, _)) => {
                self.withdrawal_done(OwnerPaymentState::Unknown);
                None
            }
            _ => None,
        };
        let names: Vec<Name> = self.peers.keys().copied().collect();
        for name in names {
            self.resync_payee(account, name, height, &before, effects);
            self.resync_payer(account, name, height, &before, funds);
        }
    }

    /// Whether our settlement that used its sequence in between applied:
    /// the hold shows its funds. Returns the hold and what it added.
    fn settled_unread(
        &mut self,
        account: &HubAccount,
        before: &BTreeMap<HoldNumber, &HoldMirror>,
        settlement: Name,
        operation: &MemberOperation,
        effects: &mut Vec<Effect>,
    ) -> Option<(HoldNumber, Amount)> {
        let settling = self.settling.remove(&settlement)?;
        let us = self.account?;
        let applied = match operation {
            MemberOperation::HoldOpen { payee, .. } => account
                .holds()
                .find(|(number, hold)| {
                    hold.payer == us && hold.payee == *payee && !before.contains_key(number)
                })
                .map(|(number, _)| number),
            MemberOperation::HoldFund { hold, .. } => {
                let was = before.get(hold).map_or(0, |old| funded(old));
                account
                    .hold(*hold)
                    .filter(|now| funded(now) >= was.saturating_add(settling.drops))
                    .map(|_| *hold)
            }
            _ => None,
        };
        let Some(hold) = applied else {
            effects.push(Effect::Cancel { settlement });
            return None;
        };
        effects.push(Effect::Finalize { settlement });
        self.booked = self.booked.saturating_sub(units(settling.drops));
        if let MemberOperation::HoldOpen { .. } = operation {
            if let Some(peer) = self.peers.get_mut(&settling.peer) {
                peer.payer.hold = Some(hold);
            }
            let reserve = units(self.terms.fees.hold_reserve);
            effects.push(Effect::Reconcile {
                amount: reserve,
                gain: false,
                cause: ReconcileCause::Reserve,
            });
            self.booked = self.booked.saturating_sub(reserve);
        }
        Some((hold, settling.drops))
    }

    /// Peer `name`'s hold toward us: what its payer added in between is
    /// its settlement. One opened in between is ours to follow from now,
    /// once the hub confirms the key the peer gave us.
    fn resync_payee(
        &mut self,
        account: &mut HubAccount,
        name: Name,
        height: u64,
        before: &BTreeMap<HoldNumber, &HoldMirror>,
        effects: &mut Vec<Effect>,
    ) {
        let (Some(us), Some(peer)) = (self.account, self.peers.get(&name)) else {
            return;
        };
        let found = peer.payee.hold.or_else(|| {
            account
                .holds()
                .find(|(_, hold)| hold.payer == peer.account && hold.payee == us)
                .map(|(number, _)| number)
        });
        let Some(number) = found else {
            return;
        };
        let key = peer.key;
        let Some(now) = account.hold(number) else {
            // Released or reduced to nothing: the account's state says what
            // that left us.
            if let Some(peer) = self.peers.get_mut(&name) {
                peer.payee = super::payee::Payee::fresh(peer.payee.key_confirmed);
            }
            return;
        };
        let grew = funded(now).saturating_sub(before.get(&number).map_or(0, |old| funded(old)));
        if let Some(peer) = self.peers.get_mut(&name)
            && peer.payee.hold.is_none()
        {
            peer.payee.hold = Some(number);
            account.look_up(key);
        }
        if grew > 0 {
            let amount = units(grew);
            effects.push(Effect::IncomingSettlement {
                peer: name,
                amount,
                id: Some(resync_id(0, number, height)),
            });
            self.booked = self.booked.saturating_add(amount);
        }
    }

    /// Our hold toward peer `name`: what the payee took in between beyond
    /// what we owed it is what the hub paid with claims we never read.
    fn resync_payer(
        &mut self,
        account: &HubAccount,
        name: Name,
        height: u64,
        before: &BTreeMap<HoldNumber, &HoldMirror>,
        funds: Option<(HoldNumber, Amount)>,
    ) {
        let Some(peer) = self.peers.get_mut(&name) else {
            return;
        };
        let Some(number) = peer.payer.hold else {
            return;
        };
        let Some(now) = account.hold(number) else {
            // Released: what it held came back to us.
            peer.payer = super::payer::Payer::new();
            return;
        };
        let old = before.get(&number);
        let added = funds
            .filter(|(hold, _)| *hold == number)
            .map_or(0, |(_, drops)| drops);
        let funded_by_now = old.map_or(0, |old| funded(old)).saturating_add(added);
        let taken_before = old.map_or(0, |old| old.taken());
        let taken_between = funded_by_now
            .saturating_sub(now.amount)
            .saturating_sub(taken_before);
        let owed = Amount::try_from(peer.payer.owed_units().div_ceil(u128::from(UNITS_PER_DROP)))
            .unwrap_or(Amount::MAX);
        let excess = taken_between.saturating_sub(owed.saturating_sub(taken_before));
        if excess == 0 {
            return;
        }
        peer.payer.through_units = peer
            .payer
            .through_units
            .saturating_add(u128::from(excess) * u128::from(UNITS_PER_DROP));
        let loss = Loss {
            units: units(excess),
            cause: LossCause::PaidLate,
            operation: Digest(resync_digest(number, height)),
            hold: number,
            sequence: 0,
        };
        self.queue_loss(name, loss);
    }
}

/// The bytes a resync's loss on `hold` is named by: one per hold and block.
fn resync_digest(hold: HoldNumber, height: u64) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let parts = hold
        .0
        .to_be_bytes()
        .into_iter()
        .chain(height.to_be_bytes())
        .chain(core::iter::once(0xee));
    for (slot, byte) in bytes.iter_mut().zip(parts) {
        *slot = byte;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_resync_books_is_named_once_per_hold_and_block() {
        let a = resync_id(0, HoldNumber(1), 2);
        assert_ne!(a, resync_id(0, HoldNumber(2), 1));
        assert_ne!(a, resync_id(0, HoldNumber(1), 3));
        assert_eq!(a.as_str().len(), 34);
        assert_ne!(
            resync_digest(HoldNumber(1), 2),
            resync_digest(HoldNumber(2), 1)
        );
    }
}
