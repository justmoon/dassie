//! A hub's escrow on the XRP Ledger: the keys validators make for it, the
//! sponsor's setup and the validators' check of it, the payments that carry
//! withdrawals out, what validators attest from its transactions, and the
//! payout set that pays everyone out at the end
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals,
//! spec/designs/settlement-hubs.md#9-shutdown).
//!
//! Native multi-signing: each validator signs alone, so signatures can be
//! gathered over days and from validators never online together.

pub mod bip32;
pub mod keys;
pub mod observe;
pub mod payout;
pub mod transactions;

pub use keys::{ValidatorKeys, escrow_master, member_key};
pub use observe::{OPEN_MEMO_TYPE, deposit_of, outcome_of, withdrawal_outcome};
pub use payout::{
    MAX_PAYOUT_BUNDLE_BYTES, PayoutBundle, PayoutEntry, PayoutError, PayoutSet, PayoutTransaction,
    assemble, payout_set, sign_payout_set,
};
pub use transactions::{
    EscrowFault, EscrowSeen, SetupStep, check_escrow, escrow_account, setup, setup_step,
    signer_entries, signer_list_set, signer_quorum, withdrawal_payment,
};
