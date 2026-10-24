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
