//! The owner's payments out of a scheme's account, such as a deposit into
//! a settlement hub, held against the owner's funds while they are on
//! their way (spec/areas/settlement.md#the-scheme-interface).
//!
//! A payment is booked as a pending transfer out of the owner's group on
//! the payment's ledger (Dr `equity/owner`, Cr `assets/settlement`) in the
//! batch that keeps the payment, and so is the most its fees may cost, apart,
//! as a second hold under the payment's ID prefixed `fee-`, so conversions, payer and BTP tokens and
//! another scheme's gas see the owner's funds less it from the moment it
//! leaves, and the owner's funds never go below zero when it lands. The
//! scheme reports once it is no longer on its way (`Effect::OwnerPaid`):
//! applied, the hold is posted; otherwise it is voided. The fee hold is
//! voided either way, as the scheme books the fee it cost. The hold outlives a
//! restart, as a settlement's does; at the start each one the scheme no
//! longer has on its way is posted or voided by what the scheme says.

use dassie_ledger::{AccountName, LedgerId, Name, Reference, TransferId, TransferSpec};
use dassie_settlement::{OwnerPaymentState, SchemeError};

use super::Core;

impl Core {
    /// The hold for a payment of `amount` out of the owner's funds on
    /// `ledger`, for `scheme`, under `id`: refused when the owner's funds do
    /// not cover it.
    fn owner_hold(
        &self,
        scheme: Name,
        ledger: LedgerId,
        amount: u64,
        id: Name,
    ) -> Result<TransferSpec, SchemeError> {
        let account = |name| {
            self.account(ledger, name)
                .map_err(|_| SchemeError::Unavailable)
        };
        Ok(TransferSpec {
            debit: account(AccountName::EquityOwner)?,
            credit: account(AccountName::AssetsSettlement)?,
            amount,
            pending: true,
            reference: Reference::OwnerPayment { scheme, id },
        })
    }

    /// Checks that the owner's funds on `scheme`'s ledger cover a payment
    /// of `amount` with fees of at most `fee`, before it is made.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] when they do not, or the scheme does not
    /// run.
    pub(super) fn check_owner_payment(
        &self,
        scheme: Name,
        amount: u64,
        fee: u64,
    ) -> Result<(), SchemeError> {
        let ledger = self.scheme_ledger(scheme)?;
        let spec = self.owner_hold(scheme, ledger, amount.saturating_add(fee), scheme)?;
        self.ledger
            .validate(&spec)
            .map_err(|_| SchemeError::Unavailable)
    }

    /// Holds the owner's funds for payment `id` of `amount`, just made, and
    /// for at most `fee` of fees, in the batch that keeps it.
    // Panics only on a broken invariant: the books checked the holds just
    // before the payment, and nothing was booked in between.
    #[allow(clippy::panic)]
    pub(super) fn hold_owner_payment(&mut self, scheme: Name, id: Name, amount: u64, fee: u64) {
        let Ok(ledger) = self.scheme_ledger(scheme) else {
            panic!("the scheme of an owner's payment just made is gone");
        };
        let holds = [(id, amount), (fee_hold(id), fee)];
        for (id, amount) in holds.into_iter().filter(|(_, amount)| *amount > 0) {
            let Ok(spec) = self.owner_hold(scheme, ledger, amount, id) else {
                panic!("the owner's accounts of a running scheme are gone");
            };
            self.take_journal();
            if let Err(error) = self.ledger.create(self.now_ms, &spec) {
                panic!("an owner's payment checked just now could not be held: {error}");
            }
        }
    }

    fn scheme_ledger(&self, scheme: Name) -> Result<LedgerId, SchemeError> {
        self.schemes
            .get(&scheme)
            .filter(|active| active.failed.is_none())
            .map(|active| active.scheme.ledger())
            .ok_or(SchemeError::Unavailable)
    }

    /// The hold of `scheme`'s payment `id`, if the books still have it
    /// pending.
    fn owner_payment_hold(&self, scheme: Name, id: Name) -> Option<TransferId> {
        self.ledger
            .pending_transfers()
            .find(|transfer| transfer.reference == Reference::OwnerPayment { scheme, id })
            .map(|transfer| transfer.id)
    }

    /// Whether `scheme` has a payment of the owner's held, on its way: the
    /// hold outlives a restart, and is resolved only once the scheme says
    /// what became of the payment.
    pub(super) fn owner_payment_held(&self, scheme: Name) -> bool {
        self.ledger.pending_transfers().any(|transfer| {
            matches!(transfer.reference, Reference::OwnerPayment { scheme: held, .. } if held == scheme)
        })
    }

    /// `scheme`'s payment `id` is no longer on its way: its hold is posted
    /// if it applied, and voided otherwise; its fee hold is voided, as the
    /// scheme books the fee. A hold the books do not have pending was
    /// resolved already, or never made.
    pub(super) fn owner_paid(&mut self, scheme: Name, id: Name, applied: bool) {
        self.resolve_hold(scheme, id, applied);
        self.resolve_hold(scheme, fee_hold(id), false);
    }

    // Panics only on a broken invariant: a pending transfer posts or voids.
    #[allow(clippy::panic)]
    fn resolve_hold(&mut self, scheme: Name, id: Name, post: bool) {
        let Some(hold) = self.owner_payment_hold(scheme, id) else {
            return;
        };
        self.take_journal();
        let resolved = if post {
            self.ledger.post(self.now_ms, hold)
        } else {
            self.ledger.void(self.now_ms, hold)
        };
        if let Err(error) = resolved {
            panic!("an owner's payment's hold could not be resolved: {error}");
        }
    }

    /// At the start, and when a scheme is activated again, resolves each
    /// held payment its scheme no longer has on its way, by what the scheme
    /// says became of it: posted if it applied, voided otherwise. One still
    /// on its way, or of a scheme that does not run, stays held; a fee hold
    /// goes with its payment's.
    pub(super) fn reconcile_owner_payments(&mut self) {
        let held: Vec<(Name, Name)> = self
            .ledger
            .pending_transfers()
            .filter_map(|transfer| match transfer.reference {
                Reference::OwnerPayment { scheme, id } if !is_fee_hold(id) => Some((scheme, id)),
                _ => None,
            })
            .collect();
        for (scheme, id) in held {
            let state = self
                .schemes
                .get(&scheme)
                .filter(|active| active.failed.is_none())
                .map(|active| active.scheme.owner_payment(id));
            match state {
                None | Some(OwnerPaymentState::InFlight) => {}
                Some(OwnerPaymentState::Applied) => self.owner_paid(scheme, id, true),
                Some(
                    OwnerPaymentState::Failed(_)
                    | OwnerPaymentState::Cancelled
                    | OwnerPaymentState::Unknown,
                ) => {
                    self.owner_paid(scheme, id, false);
                }
            }
        }
    }
}

/// The ID of the hold on what a payment's fees may cost: its own, prefixed,
/// so that no payment's ID is one.
fn fee_hold(id: Name) -> Name {
    let text = id.as_str();
    let kept = text.get(..text.len().min(60)).unwrap_or(text);
    Name::new(&format!("fee-{kept}")).unwrap_or(id)
}

/// Whether `id` names the hold on a payment's fees.
fn is_fee_hold(id: Name) -> bool {
    id.as_str().starts_with("fee-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fee_hold_is_named_apart_from_its_payment() {
        let hash = Name::hex(&[0xab; 32], true);
        let fee = fee_hold(hash);
        assert!(is_fee_hold(fee) && !is_fee_hold(hash));
        assert_ne!(fee, hash);
        let withdrawal = Name::hex(&7u64.to_be_bytes(), false);
        assert_eq!(fee_hold(withdrawal).as_str(), "fee-0000000000000007");
    }
}
