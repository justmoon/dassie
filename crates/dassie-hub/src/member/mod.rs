//! A member's side of a settlement hub (spec/areas/hubs.md#the-members-side,
//! spec/designs/settlement-hubs.md#4-peering-over-a-hub), sans-IO.
//!
//! - [`HubAccount`] is the member's account in the hub as an external
//!   asset (spec/areas/settlement.md#external-assets): it talks to the
//!   hub's validators, trusts what f + 1 of them sign alike, reads what was
//!   executed in the order of the log once a quorum vouched for every
//!   block up to it, mirrors the holds it pays into or takes from, and
//!   submits the member's operations one at a time.
//! - [`HubScheme`] is the settlement scheme on it, named by the hub's
//!   network and asset (`hub-xrpl-testnet-xrp`, `hub-xrpl-testnet-dsusd`,
//!   and on mainnet `hub-xrpl-xrp`, `hub-xrpl-rlusd`; [`scheme_of`]): it peers with members of the same hub, settles by
//!   funding the hold toward a peer, and signs and checks the promises each
//!   packet carries ([`Commitments`](dassie_settlement::Commitments)).
//!
//! How hub money is booked: a hold is the credit its payee extends, so
//! funding one is the payer's outgoing settlement and the payee's incoming
//! one, which the payee books once agreed statements show the hold toward
//! it grew. What a payee takes from a hold, with a redeem or a claim, moves
//! within its own settlement asset, and books nothing but the fee.

pub mod account;
pub mod mainnet;
mod mirror;
mod payee;
mod payer;
pub mod payout;
mod peering;
mod resync;
pub mod scheme;
mod state;

pub use account::{FIRST, HubAccount, News, Purpose};
pub use mainnet::{DEFAULT_MAINNET_VALIDATORS, ListedValidator, check_default_hub};
pub use mirror::HoldMirror;
pub use payout::{Payouts, Taken};
pub use scheme::{
    DSUSD_SCHEME_ID, HubScheme, MAINNET_SCHEME_ID, RLUSD_SCHEME_ID, SCHEME_ID, scheme_of,
};
pub use state::hub_of_state;

pub use crate::rules::LEAST_ESCROW_GAS_DROPS;

/// How long a member's check that a token hub's escrow holds
/// [`LEAST_ESCROW_GAS_DROPS`] holds for the withdrawal it was made for: long
/// enough for an owner told to ask again shortly to do so, and short
/// against what the escrow's XRP may lose meanwhile, which only the fees of
/// its own transactions burn, each a small part of the least gas
/// (spec/decisions/token-hubs.md).
pub const GAS_CHECK_LASTS_MS: u64 = 60_000;

/// The default hub of the XRP Ledger testnet, as the hex of its rules'
/// encoding, kept like the default bootstrap node list
/// (spec/designs/settlement-hubs.md#11-choosing-a-hub): founded by boot1
/// with boot1–4 as its validators. A node names another hub in its
/// scheme's config.
///
/// None is built in until the testnet's hub is founded again under the
/// current rules and with its keys at the paths that name its network
/// (spec/decisions/hub-keys-by-network.md). The one founded before, its
/// margin and its encoding changed, no longer decodes or checks, and it
/// names validator keys its validators no longer derive. Meanwhile the
/// wallet does not offer the scheme, and a scheme added without rules of
/// its own does not start (spec/areas/hubs.md#in-the-node). Founding it
/// again puts its rules' hex in `default-testnet-hub.hex` and this back to
/// `Some(include_str!("default-testnet-hub.hex"))`.
pub const DEFAULT_TESTNET_HUB: Option<&str> = None;

/// The default DSUSD hub of the XRP Ledger testnet, as the hex of its
/// rules' encoding, kept as [`DEFAULT_TESTNET_HUB`] is: founded by boot1
/// with boot1–4 as its validators, holding DSUSD
/// (spec/decisions/token-hubs.md). A node names another DSUSD hub in its
/// scheme's config.
///
/// None is built in until boot1 founds it under the current rules: an
/// operator's step, which puts its rules' hex in
/// `default-testnet-dsusd-hub.hex` and this to
/// `Some(include_str!("default-testnet-dsusd-hub.hex"))`. Meanwhile the
/// wallet does not offer `hub-xrpl-testnet-dsusd`, and the scheme added
/// without rules of its own does not start (spec/areas/hubs.md#in-the-node).
pub const DEFAULT_TESTNET_DSUSD_HUB: Option<&str> = None;

/// The default hub of XRP on the XRP Ledger's mainnet, as the hex of its
/// rules' encoding, kept as [`DEFAULT_TESTNET_HUB`] is: founded by the
/// project with the validators it runs on mainnet, each on the default
/// list of mainnet validators ([`DEFAULT_MAINNET_VALIDATORS`]), holding
/// real XRP. A node joins it only once its owner turns hubs on, by adding
/// `hub-xrpl-xrp`; one names another mainnet hub in the scheme's config.
///
/// None is built in until the project's mainnet validators found it: an
/// operator's step, which puts its rules' hex in `default-mainnet-hub.hex`
/// and this to `Some(include_str!("default-mainnet-hub.hex"))`, once
/// [`check_default_hub`] passes it against the list, as a test checks.
/// Meanwhile the wallet does not offer `hub-xrpl-xrp`, and the scheme added
/// without rules of its own does not start (spec/areas/hubs.md#in-the-node).
pub const DEFAULT_MAINNET_HUB: Option<&str> = None;

/// The default hub of RLUSD on the XRP Ledger's mainnet, kept as
/// [`DEFAULT_MAINNET_HUB`] is: founded by the project with its mainnet
/// validators, holding RLUSD of its issuer (spec/decisions/token-hubs.md).
///
/// None is built in until they found it: an operator's step, which puts
/// its rules' hex in `default-mainnet-rlusd-hub.hex` and this to
/// `Some(include_str!("default-mainnet-rlusd-hub.hex"))`, once
/// [`check_default_hub`] passes it. Meanwhile the wallet does not offer
/// `hub-xrpl-rlusd`, and the scheme added without rules of its own does
/// not start.
pub const DEFAULT_MAINNET_RLUSD_HUB: Option<&str> = None;

/// The account a node's account in the hub of scheme `scheme` is, as an
/// external asset: on the XRP Ledger testnet, [`FIRST`] in its XRP hub and
/// the next in its DSUSD hub; on mainnet, a hub account of the live realm's
/// kind, the first in its XRP hub and the next in its RLUSD hub, so that a
/// mainnet account's ID says it holds real money. A node is in one hub per
/// asset (spec/decisions/hubs-only.md), so the scheme names the account;
/// its key is derived for the hub it is in ([`member_key`]), so the number
/// is only its place among the node's accounts. `None` for a scheme that is
/// no hub's.
#[must_use]
pub fn account_id(scheme: &str) -> Option<dassie_settlement::AssetId> {
    use dassie_settlement::AssetKind;

    let (kind, index) = match scheme {
        SCHEME_ID => (FIRST.kind, FIRST.index),
        DSUSD_SCHEME_ID => (FIRST.kind, FIRST.index + 1),
        MAINNET_SCHEME_ID => (AssetKind::HubAccount, 0),
        RLUSD_SCHEME_ID => (AssetKind::HubAccount, 1),
        _ => return None,
    };
    Some(dassie_settlement::AssetId { kind, index })
}

use dassie_identity::{NodeKey, NodeSecret};
use dassie_ledger::{MAX_TOTAL, Name};

use crate::digest::Digest;
use crate::rules::Amount;

/// A member's account key in the hub `hub`, at
/// `node/hub/member/<hub ID>/account`: one key per hub, so accounts in two
/// hubs are not linked by their key. The path needs no network: the hub ID
/// is the hash of rules whose terms name it, so hubs on two networks never
/// share an ID.
#[must_use]
pub fn member_key(node: &NodeKey, hub: &crate::rules::HubId) -> NodeKey {
    let secret = node.secret(NodeSecret::HubMember {
        hub: hub.0.as_bytes(),
    });
    NodeKey::from_bytes(secret.as_bytes())
}

/// Internal units per unit of a hub's asset: the books count XRP and a
/// dollar token at scale 9, a hub in its asset's smallest unit, drops of
/// XRP or millionths of a token ([`TOKEN_DECIMALS`](crate::rules::TOKEN_DECIMALS)),
/// at scale 6. So one rate serves both, and what is said here in drops
/// holds for a token hub's millionths.
pub const UNITS_PER_DROP: u64 = dassie_xrpl::UNITS_PER_DROP;

const _: () = assert!(
    UNITS_PER_DROP == dassie_xrpl::token::UNITS_PER_MICRO
        && UNITS_PER_DROP == 10u64.pow(9 - crate::rules::TOKEN_DECIMALS as u32),
    "a drop and a token's unit are the same in the books"
);

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
