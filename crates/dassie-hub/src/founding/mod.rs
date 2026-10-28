//! Founding a hub (spec/designs/settlement-hubs.md#2-founding-a-hub), as
//! sans-IO code: what a sponsor and the validators it picked say to each
//! other, what a validator decides, and where a sponsor's founding stands.
//!
//! 1. The sponsor proposes its terms to each validator. A validator whose
//!    owner lists the sponsor, and that validates fewer hubs than its own
//!    limit, makes its keys under the terms' nonce and answers its signed
//!    acceptance ([`validator::accept`]).
//! 2. Once every validator accepted, the sponsor fixes the rules (the
//!    terms and the acceptances) and a genesis timestamp, and asks each to
//!    sign the genesis. A validator signs at most one genesis per nonce,
//!    so its acceptance cannot found two hubs with different co-validators
//!    ([`validator::sign_genesis`]).
//! 3. A quorum's signatures over the genesis are the genesis certificate
//!    ([`GenesisCertificate`]). The sponsor hands it to every validator,
//!    which runs the hub once it checks ([`validator::take_certificate`]).
//! 4. The sponsor then sets up the escrow on the XRP Ledger
//!    ([`crate::escrow::setup_step`]), and the validators check it, as for
//!    any hub, before the first deposit.
//!
//! [`sponsor::Sponsor`] keeps where the sponsor's founding stands and what
//! it asks next; the node carries the requests over the hub's founding
//! stream and keeps the state.

pub mod sponsor;
pub mod validator;
pub mod wire;

pub use sponsor::{Invited, Sponsor, Stage};
pub use validator::{Policy, Record};
pub use wire::{FoundingAnswer, FoundingRequest, GenesisCertificate, Refusal};

use crate::Millis;
use crate::digest::{Digest, Domain};
use crate::rules::Rules;
use crate::service::genesis_block;

/// How far ahead of a validator's clock a genesis timestamp may be: the
/// sponsor stamps it, and clocks differ a little.
pub const MAX_GENESIS_AHEAD_MS: Millis = 60_000;

/// How old a genesis timestamp may be when a validator first signs it: a
/// founding that waited a day for its last validator starts again with a
/// genesis of its own.
pub const MAX_GENESIS_AGE_MS: Millis = 86_400_000;

/// How long a validator keeps an acceptance whose genesis it was never
/// asked to sign: past it, the place the acceptance held is free again.
pub const ACCEPTANCE_TTL_MS: Millis = 86_400_000;

/// What a validator's consensus key signs for the genesis of the hub of
/// `rules` stamped `genesis_ms`: the genesis block, under a domain of its
/// own, so the signature counts for nothing else.
#[must_use]
pub fn genesis_digest(rules: &Rules, genesis_ms: Millis) -> Digest {
    let hub = rules.id();
    let block = genesis_block(rules, genesis_ms).digest(&hub.0);
    Digest::of(Domain::Genesis, &[hub.0.as_bytes(), block.as_bytes()])
}

/// The terms a sponsor proposes unless it says otherwise: the starting
/// values of the design, which the simulation is to set
/// (spec/designs/settlement-hubs.md), with its escrow at `escrow` on
/// `network`. The ledger fee covers every validator of the largest set
/// signing, at twice the XRP Ledger's base fee. The margin is the least the
/// smallest set may have; a sponsor of more validators raises it to theirs
/// (`rules::least_margin_ms`), as `Rules::check` asks.
#[must_use]
pub fn starting_terms(
    nonce: [u8; 32],
    sponsor: dassie_identity::NodeIdentity,
    network: u32,
    escrow: [u8; crate::rules::LEDGER_ACCOUNT_LEN],
) -> crate::rules::Terms {
    use crate::rules::{Fees, Limits, Terms, Timers};
    let signers = u64::try_from(crate::validators::MAX_VALIDATORS).unwrap_or(u64::MAX);
    Terms {
        version: crate::rules::HUB_VERSION,
        nonce,
        sponsor,
        network,
        escrow,
        asset: crate::rules::Asset::Xrp,
        fees: Fees {
            operation: 10,
            ledger: 20 * (signers + 1),
            gas: 0,
            reserve_share_ppm: 100_000,
            account_reserve: 2_000_000,
            hold_reserve: 200_000,
            starting_balance: 1_000,
            least_payout: 1_000_000,
        },
        limits: Limits {
            accounts: 1_000,
            holds: 4_000,
            holds_per_account: 64,
            withdrawals: 256,
            pending_deposits: 256,
            recent_deposits: 4_096,
            block_operations: 512,
            deadline_operations: 128,
            claimed_per_hold: 1_100,
            commitments_per_claim: 64,
            vote_window: 32,
        },
        timers: Timers {
            hold_notice: 60_000,
            shutdown_notice: 3_600_000,
            reconfigure_notice: 86_400_000,
            absence: 3 * 86_400_000,
            stall: 3_600_000,
            margin: crate::rules::least_margin_ms(crate::validators::MIN_VALIDATORS),
            allowance: 300,
        },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_starting_terms_check() {
        let sponsor = crate::rules::tests::node_key(1).identity();
        let terms = super::starting_terms([2; 32], sponsor, 1, [3; 20]);
        assert_eq!(terms.check(), Ok(()));
        // spec/areas/hubs.md#consensus: f + 1 views at most before an
        // honest leader's block, of four validators.
        assert_eq!(terms.timers.margin, 600);
    }
}
