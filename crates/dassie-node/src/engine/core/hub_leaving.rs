//! Leaving a settlement hub that ended, so that its scheme can be added
//! again with another hub's rules (spec/areas/hubs.md#leaving-a-hub-that-ended,
//! spec/decisions/leaving-a-hub-that-ended.md).
//!
//! - **A hub ended once its payout set is signed.** The node keeps each
//!   validator's payout bundle of its hubs (`hub/payouts.rs`); once a
//!   quorum's sign one set, it tells the core ([`Core::hub_paid_out`]), and
//!   the account knows its hub ended for good: its scheme can no longer
//!   receive, the node moves nothing into or out of it, and the owner may
//!   leave it. The bundles are in the database, so after a restart the
//!   account knows it again at once, without a stall's wait.
//! - **Leaving** ([`Core::leave_hub`]) is refused for a hub that has not
//!   ended so: closing an account in a hub that runs is not built. It
//!   forgets the move on its way, which the hub will never credit or
//!   execute, and removes the scheme as the owner's removal does, winding
//!   its methods down; once it goes, the node lets go of the hub's state
//!   ([`Core::let_go_of_hub`]).
//! - **Letting go** cancels our settlements and voids the owner's payments
//!   still pending on the hub's ledger, writes off through suspense what
//!   the books hold of the account, and forgets the scheme's checkpoint,
//!   its account's and the hub's funding. The hub's payout set pays the
//!   account to the owner book's account, which books it as the owner's
//!   deposit when it arrives, before the node left or after: nothing is
//!   counted twice.
//! - **Books out of step** ([`Core::mend_hub_books`]): a hub scheme that
//!   starts while the books of its ledger hold what its checkpoint does not
//!   explain, as after a move to another hub by hand that left the old
//!   hub's books, has them reconciled to the checkpoint through suspense.

use dassie_ledger::{AccountName, LedgerId, Name, Reference};
use dassie_settlement::AssetId;

use super::events::SettlementEvent;
use super::hub_events::MoveStage;
use super::{Core, EngineError, RemovalBlocker};
use crate::store::Checkpoint;

impl Core {
    /// The node keeps a quorum's payout set of hub `hub`: it ended for
    /// good. Its account in the hub, if the node has one, knows it from
    /// now on, and its scheme can no longer receive. Told after every
    /// round of the payout keeper, so again after a restart; a hub the node
    /// is in no more is ignored.
    pub fn hub_paid_out(&mut self, hub: dassie_hub::HubId) {
        let ended: Vec<AssetId> = self
            .assets
            .iter_mut()
            .filter_map(|(id, active)| {
                let any: &mut dyn core::any::Any = active.asset.as_mut();
                let account = any.downcast_mut::<dassie_hub::member::HubAccount>()?;
                if account.hub() != hub || account.paid_out() {
                    return None;
                }
                account.end_by_payout();
                Some(*id)
            })
            .collect();
        for asset in ended {
            let schemes: Vec<Name> = self
                .schemes
                .iter()
                .filter(|(_, active)| active.asset == Some(asset))
                .map(|(name, _)| *name)
                .collect();
            for scheme in schemes {
                self.report(SettlementEvent::HubEnded {
                    scheme,
                    hub: *hub.0.as_bytes(),
                });
            }
            // Its scheme hears it as it hears what the account learnt.
            self.asset_heard(asset);
            self.changes.ledger = true;
        }
    }

    /// The owner leaves the hub over `scheme`, which ended: a quorum of its
    /// validators signed its payout set, which the node keeps
    /// ([`hub_paid_out`](Self::hub_paid_out)). The move on its way, if
    /// any, is forgotten, and the scheme is removed as by
    /// [`remove_scheme`](Self::remove_scheme); once it goes, the node lets
    /// go of the hub's state, so the scheme is added again, with another
    /// hub's rules, as one never added. Leaving a hub being left succeeds,
    /// and changes nothing. Returns whether the scheme went at once;
    /// otherwise it goes once its methods are wound down.
    ///
    /// # Errors
    ///
    /// No such hub; the hub has not ended, as far as the node knows; or BTP
    /// tokens or the owner's unfinished payments use its ledger.
    pub fn leave_hub(&mut self, scheme: &str) -> Result<bool, EngineError> {
        let scheme = self.chosen_hub(scheme)?;
        // Leaving a hub being left succeeds, even before the payout keeper
        // has told the engine again after a restart that the hub ended.
        if self
            .schemes
            .get(&scheme)
            .is_some_and(|active| active.leaving)
        {
            return Ok(false);
        }
        let account = self
            .hub_account(scheme)
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))?;
        if !account.paid_out() {
            return Err(EngineError::Precondition(format!(
                "the hub {} has not ended, as far as the node knows: a hub is left once a quorum \
                 of its validators signed its payout set, which `dassie hub payout {scheme}` \
                 shows; leaving a hub that runs is not built",
                account.hub().0
            )));
        }
        let active = self
            .schemes
            .get(&scheme)
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))?;
        let ledger = active.scheme.ledger();
        // The move on its way never keeps the node in a hub that ended: it
        // is forgotten below.
        if let Some(blocker) = self
            .removal_blockers(scheme, ledger)
            .into_iter()
            .find(|blocker| *blocker != RemovalBlocker::HubMove)
        {
            return Err(EngineError::Precondition(blocker.message().into()));
        }
        self.follow_hubs();
        let forgotten = self.funding.get_mut(&scheme).and_then(|f| f.forget());
        if let Some(forgotten) = forgotten {
            self.tap_move(scheme, MoveStage::GivenUp, &forgotten);
            self.keep_funding(scheme);
        }
        let removing = match self.schemes.get_mut(&scheme) {
            Some(active) => {
                active.leaving = true;
                active.removing.is_some()
            }
            None => return Err(EngineError::Internal("the hub scheme vanished".into())),
        };
        self.batch.schemes_leaving.push(scheme);
        self.changes.ledger = true;
        // A removal under way carries on, and lets go once it is done.
        if !removing {
            self.start_removal(scheme, ledger);
        }
        Ok(!self.schemes.contains_key(&scheme))
    }

    /// Lets go of the hub of scheme `id`, on `ledger`, which the owner
    /// leaves, as the scheme goes: what is still pending on the ledger is
    /// resolved, the books' holding of the account is written off through
    /// suspense, and the scheme's checkpoint, its account's and the hub's
    /// funding are forgotten. The hub's payout set pays the account to the
    /// owner book's account, which books it as the owner's deposit when it
    /// arrives; its accounts with peers on the ledger keep what they show,
    /// as for any method dropped, for a later method on the scheme to
    /// settle.
    // Panics only on a broken invariant: a reconciliation, a report, is
    // booked whatever the limits, between accounts the ledger has.
    #[allow(clippy::panic)]
    pub(super) fn let_go_of_hub(&mut self, id: Name, ledger: LedgerId) {
        let asset = self.schemes.get(&id).and_then(|active| active.asset);
        // Its account's, whether or not the books stopped the scheme
        // meanwhile.
        let hub = asset
            .and_then(|asset| self.assets.get(&asset))
            .and_then(|active| {
                let any: &dyn core::any::Any = active.asset.as_ref();
                any.downcast_ref::<dassie_hub::member::HubAccount>()
            })
            .map_or([0; 32], |account| *account.hub().0.as_bytes());
        // Our settlements over the hub never reach the peer now: what
        // funded a hold in it goes back to us with the payout set.
        let settlements: Vec<Name> = self
            .ledger
            .pending_transfers()
            .filter_map(|transfer| match transfer.reference {
                Reference::Settlement {
                    scheme,
                    id: settlement,
                } if scheme == id => Some(settlement),
                _ => None,
            })
            .collect();
        for settlement in settlements {
            self.settlement_cancelled(id, settlement);
        }
        // The owner's withdrawals the hub will not execute now; one it
        // executed is in the payout set, and the owner book books it as it
        // arrives.
        self.void_owner_payments(id);
        let written_off = self.write_off_settlement(ledger);
        self.report(SettlementEvent::HubLeft {
            scheme: id,
            hub,
            written_off,
        });
        self.checkpoints.remove(&id);
        self.funding.remove(&id);
        self.batch.checkpoints.retain(|(scheme, _)| *scheme != id);
        self.batch.hub_funding.remove(&id);
        self.batch.hub_automatic.remove(&id);
        self.batch.hub_target.remove(&id);
        let asset_text = asset.map_or_else(|| id.as_str().to_string(), |asset| asset.to_string());
        if let Some(asset) = asset {
            self.asset_checkpoints.remove(&asset);
        }
        self.batch
            .asset_checkpoints
            .retain(|(of, _)| *of != asset_text);
        self.batch.schemes_let_go.push((id, asset_text));
    }

    /// Writes off through suspense what the books hold on `ledger`'s
    /// `assets/settlement`, posted: Dr `equity/suspense`, Cr
    /// `assets/settlement`, as a reconciliation. Returns how much.
    // Panics only on a broken invariant, as `let_go_of_hub`.
    #[allow(clippy::panic)]
    fn write_off_settlement(&mut self, ledger: LedgerId) -> u64 {
        let held = self.settlement_held(ledger);
        let (Ok(suspense), Ok(settlement)) = (
            self.account(ledger, AccountName::EquitySuspense),
            self.account(ledger, AccountName::AssetsSettlement),
        ) else {
            return 0;
        };
        if let Err(error) = self.post(suspense, settlement, held, Reference::Reconciliation) {
            panic!("a hub's holding could not be written off: {error}");
        }
        held
    }

    /// What the books hold on `ledger`'s `assets/settlement`, posted:
    /// what stands for the holding its scheme settles in.
    pub(super) fn settlement_held(&self, ledger: LedgerId) -> u64 {
        self.account(ledger, AccountName::AssetsSettlement)
            .ok()
            .and_then(|id| self.ledger.account(id))
            .map_or(0, |account| {
                let balance = account.balance();
                let held = balance.debits_posted.saturating_sub(balance.credits_posted);
                u64::try_from(held).unwrap_or(u64::MAX)
            })
    }

    /// Before a hub scheme `id` on `ledger` starts from `checkpoint`:
    /// reconciles through suspense what the books hold of its account
    /// beyond, or short of, what the checkpoint says, nothing for none. The
    /// books keep `assets/settlement` in step with the scheme's balance,
    /// which the checkpoint keeps in the batch that books it, so the two
    /// differ only where something wrote one without the other, as an
    /// operator who moved the scheme to another hub by hand, deleting its
    /// checkpoint but not the books of the hub before: the books would
    /// count that hub's money as well as the payout that returned it.
    /// Reported as a warning. Only a scheme over a hub, whose ledger is its
    /// own.
    ///
    /// # Errors
    ///
    /// If the books refuse the reconciliation, which they book whatever
    /// the limits.
    pub(super) fn mend_hub_books(
        &mut self,
        id: Name,
        ledger: LedgerId,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<(), EngineError> {
        if dassie_hub::member::account_id(id.as_str()).is_none() {
            return Ok(());
        }
        let held = self.settlement_held(ledger);
        let kept = checkpoint.map_or(0, |checkpoint| checkpoint.balance);
        if held == kept {
            return Ok(());
        }
        let suspense = self.account(ledger, AccountName::EquitySuspense)?;
        let settlement = self.account(ledger, AccountName::AssetsSettlement)?;
        let (debit, credit) = if held > kept {
            (suspense, settlement)
        } else {
            (settlement, suspense)
        };
        self.post(
            debit,
            credit,
            held.abs_diff(kept),
            Reference::Reconciliation,
        )?;
        self.report(SettlementEvent::HubBooksMended {
            scheme: id,
            held,
            kept,
        });
        Ok(())
    }
}
