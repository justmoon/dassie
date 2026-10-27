//! The credit we extend to a peer, booked as trust transfers between its
//! `contra/trust` and `assets/interledger` accounts
//! (spec/areas/settlement.md#credit).

use dassie_ledger::{
    AccountName, AccountPath, Ledger, LedgerId, Name, Reference, TransferError, TransferSpec,
};

/// Raises the credit we extend to `node` on `ledger` by `amount`: a trust
/// transfer, as when it was first extended. Over a settlement hub the credit
/// follows our holds toward the node
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub); with unlimited
/// trust it is raised to the unlimited credit
/// (spec/areas/settlement.md#the-stub-scheme).
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

/// Lowers the credit we extend to `node` on `ledger` by `amount`: a trust
/// transfer back, as [`raise_trust`] books one. The ledger refuses it past
/// what the peer may still owe us, or past the credit.
pub(super) fn lower_trust(
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
                debit: interledger,
                credit: contra,
                amount,
                pending: false,
                reference: Reference::Trust,
            },
        )
        .map(|_| ())
}
