//! A member's side of a settlement hub (spec/areas/hubs.md#the-members-side,
//! spec/designs/settlement-hubs.md#4-peering-over-a-hub), sans-IO.
//!
//! - [`HubAccount`] is the member's account in the hub as an external
//!   asset (spec/areas/settlement.md#external-assets): it talks to the
//!   hub's validators, trusts what f + 1 of them sign alike, reads what was
//!   executed in the order of the log once a quorum vouched for every
//!   block up to it, mirrors the holds it pays into or takes from, and
//!   submits the member's operations one at a time.
//! - [`HubScheme`] is the `hub-xrpl-testnet-xrp` settlement scheme on it: it
//!   peers with members of the same hub, settles by funding the hold toward
//!   a peer, and signs and checks the promises each packet carries
//!   ([`Commitments`](dassie_settlement::Commitments)).
//!
//! How hub money is booked: a hold is the credit its payee extends, so
//! funding one is the payer's outgoing settlement and the payee's incoming
//! one, which the payee books once agreed statements show the hold toward
//! it grew. What a payee takes from a hold, with a redeem or a claim, moves
//! within its own settlement asset, and books nothing but the fee.

pub mod account;
mod mirror;
mod payee;
mod payer;
pub mod payout;
mod peering;
mod resync;
pub mod scheme;
mod state;

pub use account::{FIRST, HubAccount, News, Purpose};
pub use mirror::HoldMirror;
pub use payout::{Payouts, Taken};
pub use peering::peer_account;
pub use scheme::{HubScheme, SCHEME_ID};

/// The default hub of the XRP Ledger testnet, as the hex of its rules'
/// encoding, kept like the default bootstrap node list
/// (spec/designs/settlement-hubs.md#11-choosing-a-hub): founded by boot1
/// with boot1–4 as its validators. A node names another hub in its
/// scheme's config.
///
/// None is built in until the testnet's hub is founded again under the
/// current rules: the one founded before their margin and their encoding
/// changed no longer decodes or checks. Meanwhile the wallet does not offer
/// the scheme, and a scheme added without rules of its own does not start
/// (spec/areas/hubs.md#in-the-node). Founding it again puts its rules'
/// hex in `default-testnet-hub.hex` and this back to
/// `Some(include_str!("default-testnet-hub.hex"))`.
pub const DEFAULT_TESTNET_HUB: Option<&str> = None;

use dassie_ledger::{MAX_TOTAL, Name};

use crate::digest::Digest;
use crate::rules::Amount;

/// Internal units per drop: the books count XRP at scale 9, the hub in
/// drops, at scale 6.
pub const UNITS_PER_DROP: u64 = dassie_xrpl::UNITS_PER_DROP;

/// The most drops the books can hold: [`MAX_TOTAL`] in units.
pub const MAX_DROPS: Amount = MAX_TOTAL / UNITS_PER_DROP;

/// `drops` in internal units. What the hub says is at most what its escrow
/// holds, far below [`MAX_DROPS`]; more is held at it, never wrapped.
#[must_use]
pub const fn units(drops: Amount) -> u64 {
    let drops = if drops > MAX_DROPS { MAX_DROPS } else { drops };
    drops * UNITS_PER_DROP
}

/// The drops that cover `units`: rounded up, so a payer's promise for a
/// packet is never less than the packet.
#[must_use]
pub const fn drops_covering(units: u64) -> Amount {
    units.div_ceil(UNITS_PER_DROP)
}

/// A digest as a name the journal keys a booking by: 64 hex digits.
#[must_use]
pub fn digest_name(digest: &Digest) -> Name {
    Name::hex(digest.as_bytes(), false)
}

#[cfg(test)]
mod tests;
