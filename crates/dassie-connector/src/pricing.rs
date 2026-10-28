//! Inventory pricing (spec/areas/connector.md#inventory-pricing,
//! spec/decisions/passive-rebalancing.md): what a node gives for what a peer
//! brings it from another ledger, moved from spot by how much of each asset
//! the owner holds, so that conversions bring a skewed inventory back toward
//! its targets without the node trading.
//!
//! The owner's funds are counted per asset, each asset whole: a ledger
//! counts with the others that name the same asset
//! ([`PriceTable::build_by_asset`]), as a settlement hub's ledger counts
//! with the owner book that funds it, between which the node moves its
//! funds by itself. For each asset that converts, `v_i` is what the owner
//! holds of it on its ledgers, valued at spot; `s_i = v_i / Σv` is its
//! share, and `t_i = 1/n` its target, for `n` assets. A unit of it, on any
//! of its ledgers, is worth `w_i = (1 − k) + k · t_i / s_i` against spot, so
//! an asset short of its target is worth more. The rate from `a` to `b` is
//! `spot(a → b) · w_a / w_b · (1 − spread)`, and between two ledgers of one
//! asset spot less the spread:
//!
//! - **1 at target:** with every share at its target, the rate is spot less
//!   the spread.
//! - **The bonus is capped** at `1 + k`: bringing in what the node lacks is
//!   rewarded, but never by more than `k` above spot.
//! - **The penalty is not:** taking out what the node lacks gets worse
//!   without bound, and out of a ledger with nothing, nothing converts,
//!   however much of its asset the owner holds on another.
//! - **Each packet pays its own impact, at the worst state in flight:** the
//!   holdings a packet is priced at are those after its own move, with every
//!   packet in flight resolved against it. Each ledger of the asset it comes
//!   in as counts every pending credit and none of its pending debits, and
//!   the ledger it comes in on the packet's own amount in; each ledger of the
//!   asset it goes out as counts every pending debit and none of its pending
//!   credits, and the ledger it goes out on less what the packet takes out
//!   at spot; every other ledger counts what is posted. So no packet held,
//!   rejected or in flight beside it, on any ledger, ever improves another's
//!   price, and a round trip never gains: its second leg is priced at a
//!   state at least as adverse as the first leg's, swapped, and at one state
//!   `w_a/w_b · w_b/w_a = 1` before the cap and the spread, which both only
//!   take away.
//!
//! Everything is exact integer arithmetic, rounding down: shares are known to
//! twelve decimals, and a rate is never better for the sender than the curve
//! at them. The table is built once per change of rates, balances or
//! settings, with the rate of a vanishingly small packet per pair; a packet
//! then reads what the owner holds on each ledger, in constant time each, and
//! is priced without allocating.
//!
//! A ledger is valued by its currency alone, whatever settles it, so a
//! settlement hub's ledger is valued as any other, and weighed as its
//! asset. Rates on different
//! ledgers are in different units; [`PriceTable::best_output`] compares them
//! relative to spot (spec/areas/connector.md#comparing-outputs).

use core::cmp::Ordering;

use dassie_ledger::{AccountId, Balance, Ledger, LedgerId, Realm};

use crate::charges::Charges;
use crate::fx::{RATE_PRECISION, Rates};

/// One unit of a rate: rates are out per in, times 10^15.
pub const RATE_ONE_E15: u64 = 1_000_000_000_000_000;

/// What a new node starts with as `k`: 5% (spec/decisions/passive-rebalancing.md),
/// a starting point.
pub const DEFAULT_REBALANCE_PPM: u32 = 50_000;

/// The most `k` may be: as the charges, 10%, so that a setting in percent
/// mistaken for parts per million cannot set the curve wild.
pub const MAX_REBALANCE_PPM: u32 = Charges::MAX_PPM;

/// Most ledgers a table prices: every ledger of one realm, since a node
/// runs the ledgers of its own realm alone, and a table prices only
/// ledgers of one ([`PriceTable::build`]). Every ledger of the realm counts,
/// a settlement hub's as much as one a node settles on directly, so a new
/// ledger raises it, and a constant assertion says when a feed could no
/// longer name them all.
pub const MAX_PRICED_LEDGERS: usize = most_ledgers_of_one_realm();

/// Most prices in a table: one per ordered pair of ledgers.
pub const MAX_PRICES: usize = MAX_PRICED_LEDGERS * (MAX_PRICED_LEDGERS - 1);

/// Parts per million.
pub(crate) const MILLION: u128 = 1_000_000;

/// How many ledgers the realm with the most has.
const fn most_ledgers_of_one_realm() -> usize {
    let (mut test, mut live) = (0, 0);
    let mut rest: &[LedgerId] = &LedgerId::ALL;
    while let [ledger, tail @ ..] = rest {
        match ledger.realm() {
            Realm::Test => test += 1,
            Realm::Live => live += 1,
        }
        rest = tail;
    }
    if test > live { test } else { live }
}

/// A whole share: each ledger's share of the owner's funds is known to
/// twelve decimal places, rounded down, which keeps every product of the
/// curve within 128 bits. A share that rounds to nothing is a ledger with
/// nothing.
pub(crate) const SHARE_ONE: u128 = 1_000_000_000_000;

/// What a unit of the common base is worth in values, at scale 9 and the
/// base's rate of 10^[`RATE_PRECISION`]: a value is `amount × VALUE_BASE /
/// value_unit`, rounded down ([`value_of`]).
pub(crate) const VALUE_BASE: u128 = 10u128.pow(RATE_PRECISION + 9);

/// The most one ledger's value counts as: so that the values of every
/// ledger add up within 128 bits. Only shares read values.
const MAX_VALUE: u128 = u128::MAX / 16;

// The bounds of `factor`'s products: 10^6 · n · 10^12 + 10^5 · 10^12 stays
// below 2^64, so a weight times a share stays below 2^104, and that times
// 1.1 · 10^6 below 2^125. The values of every ledger add up within 128 bits.
const _: () = assert!(
    MAX_PRICED_LEDGERS <= 16,
    "the curve's products fit 128 bits"
);
const _: () = assert!(
    MAX_REBALANCE_PPM <= 100_000,
    "the curve's products fit 128 bits"
);

/// What the owner sets for transit: the charges, and the curve's `k`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pricing {
    /// The fee and the spread on what passes from one peer to another.
    pub charges: Charges,
    /// `k`, in parts per million: how far the curve moves a rate from spot
    /// for a skewed inventory. Zero converts at spot, less the spread.
    pub rebalance_ppm: u32,
}

impl Pricing {
    /// Nothing charged, and no curve.
    pub const NONE: Self = Self {
        charges: Charges::NONE,
        rebalance_ppm: 0,
    };

    /// Whether the charges and `k` are within their bounds.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.charges.is_valid() && self.rebalance_ppm <= MAX_REBALANCE_PPM
    }
}

/// What the owner's group holds on one ledger, in its units: what is
/// posted, and what the packets in flight would add or take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Holding {
    /// Posted credits less posted debits, or nothing if below.
    pub posted: u128,
    /// Debits of pending transfers: what packets in flight would take out.
    pub debits_pending: u128,
    /// Credits of pending transfers: what packets in flight would bring in.
    pub credits_pending: u128,
}

impl Holding {
    /// Nothing at all.
    pub const NONE: Self = Self {
        posted: 0,
        debits_pending: 0,
        credits_pending: 0,
    };

    /// The holding a group's balance says: posted, and pending as the
    /// outside sees it.
    #[must_use]
    pub const fn of(balance: &Balance) -> Self {
        Self {
            posted: balance.credits_posted.saturating_sub(balance.debits_posted),
            debits_pending: balance.debits_pending,
            credits_pending: balance.credits_pending,
        }
    }

    /// The most it can come to: every packet in flight that brings the
    /// asset in fulfilled, every one that takes it out rejected.
    #[must_use]
    pub const fn most(&self) -> u128 {
        self.posted.saturating_add(self.credits_pending)
    }

    /// The least it can come to: every packet in flight that takes the
    /// asset out fulfilled, every one that brings it in rejected.
    #[must_use]
    pub const fn least(&self) -> u128 {
        self.posted.saturating_sub(self.debits_pending)
    }
}

/// What the owner holds on each of a table's ledgers, in the table's order.
pub type Holdings = [Holding; MAX_PRICED_LEDGERS];

/// Targets the owner sets per asset, in place of the curve's equal ones:
/// what a periphery node's own swaps aim for
/// (spec/decisions/peer-swaps.md). Each names an asset as the table does,
/// by a ledger ([`LedgerPrice::asset`]), once, with a share in parts per
/// million, more than nothing. The shares are weights: each counts against
/// their sum over the assets the table prices, so they need not add up to
/// a million.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    targets: [Option<(LedgerId, u32)>; MAX_PRICED_LEDGERS],
}

impl Targets {
    /// No targets.
    pub const NONE: Self = Self {
        targets: [None; MAX_PRICED_LEDGERS],
    };

    /// The targets `entries` names, or `None` if they are more than a table
    /// prices, name an asset twice, or give one a share of nothing or more
    /// than a million.
    #[must_use]
    pub fn new(entries: &[(LedgerId, u32)]) -> Option<Self> {
        if entries.len() > MAX_PRICED_LEDGERS {
            return None;
        }
        let mut targets = Self::NONE;
        for (index, &(asset, ppm)) in entries.iter().enumerate() {
            let twice = entries
                .get(..index)
                .unwrap_or_default()
                .iter()
                .any(|(other, _)| *other == asset);
            if twice || ppm == 0 || u128::from(ppm) > MILLION {
                return None;
            }
            if let Some(slot) = targets.targets.get_mut(index) {
                *slot = Some((asset, ppm));
            }
        }
        Some(targets)
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.targets.iter().all(Option::is_none)
    }

    /// Every target: the asset and its share, in parts per million.
    pub fn iter(&self) -> impl Iterator<Item = (LedgerId, u32)> + '_ {
        self.targets.iter().flatten().copied()
    }

    /// The share `asset` is set, if it has one.
    #[must_use]
    pub fn get(&self, asset: LedgerId) -> Option<u32> {
        self.iter()
            .find(|(target, _)| *target == asset)
            .map(|(_, ppm)| ppm)
    }
}

/// A ledger to price, with the owner's group on it and what that holds.
/// Which asset it counts toward is [`PriceTable::build_by_asset`]'s to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inventory {
    /// The ledger.
    pub ledger: LedgerId,
    /// The root of the owner's group on it, `equity/owner`: what a packet
    /// reads what the owner holds from.
    pub owner: AccountId,
    /// What the owner's group holds on it.
    pub holding: Holding,
}

/// A ledger the table prices: its spot rate, and what the owner holds on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerPrice {
    /// The ledger.
    pub ledger: LedgerId,
    /// The root of the owner's group on it.
    pub owner: AccountId,
    /// The asset its holding counts toward, named by a ledger: itself, or
    /// for a settlement hub's ledger the owner book that funds the hub.
    /// The ledgers that name the same count as one holding, with one share
    /// and one target ([`PriceTable::build_by_asset`]).
    pub asset: LedgerId,
    /// How many units of its asset the common base buys, times
    /// 10^[`RATE_PRECISION`], as [`Rates`] has it.
    pub rate: u128,
    /// When that rate was learned.
    pub as_of_ms: u64,
    /// What the owner's group held on it when the table was built.
    pub holding: Holding,
}

/// What the node gives, on transit from one peer to another, for a unit
/// that comes in on `from`, in units of `to`, after the fee, as of the
/// table: the rate of a vanishingly small packet at the worst state the
/// packets then in flight could leave. A packet gets that or less: its own
/// impact, and what moved since.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price {
    /// The ledger the packet comes in on.
    pub from: LedgerId,
    /// The ledger it goes out on.
    pub to: LedgerId,
    /// Out per in, times [`RATE_ONE_E15`]: spot, moved by the curve, less
    /// the spread, rounded down. Never zero: a pair without a rate has no
    /// price.
    pub rate_e15: u64,
    /// When the older of its two rates was learned: it is good while that is.
    pub as_of_ms: u64,
}

impl Price {
    /// What `net` converts to at this rate: `floor(net × rate_e15 / 10^15)`,
    /// or `None` when that does not fit.
    #[must_use]
    pub fn convert(&self, net: u64) -> Option<u64> {
        convert_at_rate(net, self.rate_e15)
    }
}

/// Of a packet's candidate output ledgers, the one it goes out on
/// ([`PriceTable::best_output`]): its own, where it can, or else the one it
/// gets the most out on relative to spot; and the rate it gets there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BestOutput {
    /// The ledger.
    pub ledger: LedgerId,
    /// Out per in, times [`RATE_ONE_E15`], as [`PriceTable::rate`] says:
    /// exactly one on the ledger the packet came in on, which converts
    /// nothing. Never zero.
    pub rate_e15: u64,
}

/// Why a packet could not be converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceError {
    /// A rate it needs is missing or stale.
    Stale,
    /// The pair has no rate: spot less the spread rounds to nothing or does
    /// not fit, as does the curve's rate, or the ledgers are one.
    NoRate,
    /// The owner holds too little on the ledger it would go out on: nothing
    /// once the packets in flight and the packet's own move are counted, or
    /// so little that the curve's rate rounds to nothing.
    NoFunds,
    /// What it converts to does not fit.
    TooLarge,
}

/// Every price the connector converts transit at, and the spot rates it
/// converts into the owner's own endpoints at, with what they were built
/// from. Fixed in size and copied whole, so that a rate feed can publish it
/// as it is: the most a packet gets per pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriceTable {
    /// When it was built.
    built_at_ms: u64,
    /// The earliest time a rate it uses ages out or comes of age: by then
    /// it must be built again.
    valid_until_ms: u64,
    pricing: Pricing,
    ledgers: [Option<LedgerPrice>; MAX_PRICED_LEDGERS],
    /// How many assets its ledgers count toward: the `n` of each target
    /// `1/n`, at most as many as the ledgers.
    assets: usize,
    prices: [Option<Price>; MAX_PRICES],
    /// The ledgers that convert nothing now ([`Self::halt`]).
    halted: [Option<LedgerId>; MAX_PRICED_LEDGERS],
}

/// A table never built: nothing charged, nothing converts, and it is due
/// to be built at once.
impl Default for PriceTable {
    fn default() -> Self {
        Self {
            valid_until_ms: 0,
            ..Self::without_rates(Pricing::NONE)
        }
    }
}

impl PriceTable {
    /// A table for `pricing` without rates: only what stays on one ledger
    /// passes, charged its fee.
    #[must_use]
    pub const fn without_rates(pricing: Pricing) -> Self {
        Self {
            built_at_ms: 0,
            valid_until_ms: u64::MAX,
            pricing,
            ledgers: [None; MAX_PRICED_LEDGERS],
            assets: 0,
            prices: [None; MAX_PRICES],
            halted: [None; MAX_PRICED_LEDGERS],
        }
    }

    /// Prices every ordered pair of the ledgers in `inventories` that have
    /// a rate learned at most `max_rate_age_ms` before `now_ms`, and not
    /// after: the others are absent, and their packets get T00. Each pair's
    /// price is the rate of a vanishingly small packet at the worst state
    /// the packets in flight can leave ([`Self::rate`] for nothing at the
    /// inventories' holdings). A pair without one, out of a ledger the
    /// owner holds nothing on or whose rate rounds to nothing or does not
    /// fit, is absent. A ledger named twice counts once, at its first.
    /// Every ledger with a currency is priced alike, whatever settles it: a
    /// settlement hub's ledger as one a node settles on directly. Each
    /// ledger is an asset of its own ([`Self::build_by_asset`]).
    ///
    /// # Panics
    ///
    /// If `pricing` is not valid: settings are checked before they are used.
    /// If `inventories` are of two realms: a node runs the ledgers of its
    /// own realm alone, so that a table never prices more than
    /// [`MAX_PRICED_LEDGERS`].
    #[must_use]
    pub fn build(
        rates: &Rates,
        inventories: &[Inventory],
        pricing: Pricing,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Self {
        Self::build_by_asset(
            rates,
            inventories,
            |ledger| ledger,
            pricing,
            now_ms,
            max_rate_age_ms,
        )
    }

    /// [`Self::build`], with each ledger's holding counted toward the asset
    /// `asset_of` names for it by a ledger: the ledgers priced that name
    /// the same count as one holding, with one share of the owner's funds
    /// and one target, `1/n` for `n` assets, so that an asset is counted
    /// whole however its funds are spread over its ledgers
    /// (spec/decisions/passive-rebalancing.md). A node names, for a
    /// settlement hub's ledger, the owner book that funds the hub, and for
    /// every other ledger the ledger itself; an asset whose ledgers are not
    /// all priced counts those that are. A pair of ledgers of one asset
    /// converts at spot less the spread, while the owner holds anything on
    /// the way out.
    ///
    /// # Panics
    ///
    /// As [`Self::build`]; and if `asset_of` names, for a ledger, one of
    /// another currency or realm: an asset's ledgers hold the same thing.
    #[must_use]
    pub fn build_by_asset(
        rates: &Rates,
        inventories: &[Inventory],
        asset_of: impl Fn(LedgerId) -> LedgerId,
        pricing: Pricing,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Self {
        assert!(pricing.is_valid(), "pricing is checked before it is used");
        let realm = inventories
            .first()
            .map(|inventory| inventory.ledger.realm());
        assert!(
            inventories
                .iter()
                .all(|inventory| Some(inventory.ledger.realm()) == realm),
            "a table prices the ledgers of one realm"
        );
        let mut table = Self::without_rates(pricing);
        table.built_at_ms = now_ms;
        let mut count = 0;
        for inventory in inventories {
            if count == MAX_PRICED_LEDGERS || table.ledger(inventory.ledger).is_some() {
                continue;
            }
            let asset = inventory.ledger.asset();
            let (Some(rate), Some(as_of_ms)) = (rates.get(asset.code), rates.as_of(asset.code))
            else {
                continue;
            };
            if as_of_ms > now_ms {
                table.valid_until_ms = table.valid_until_ms.min(as_of_ms);
                continue;
            }
            if now_ms - as_of_ms > max_rate_age_ms {
                continue;
            }
            let stale_at_ms = as_of_ms.saturating_add(max_rate_age_ms).saturating_add(1);
            table.valid_until_ms = table.valid_until_ms.min(stale_at_ms);
            // A rate too large for its scale values nothing: left out.
            if value_unit(rate, asset.scale).is_none() {
                continue;
            }
            let counts_with = asset_of(inventory.ledger);
            assert!(
                counts_with.asset().code == asset.code
                    && counts_with.realm() == inventory.ledger.realm(),
                "an asset's ledgers hold one currency of one realm"
            );
            // Another asset, unless a ledger before it counts toward it.
            let new = !table.ledgers().any(|entry| entry.asset == counts_with);
            if let Some(slot) = table.ledgers.get_mut(count) {
                *slot = Some(LedgerPrice {
                    ledger: inventory.ledger,
                    owner: inventory.owner,
                    asset: counts_with,
                    rate,
                    as_of_ms,
                    holding: inventory.holding,
                });
                count += 1;
                table.assets += usize::from(new);
            }
        }
        assert!(table.assets <= count && count <= MAX_PRICED_LEDGERS);
        table.price_pairs();
        table
    }

    /// Fills the price of every ordered pair of the table's ledgers: the
    /// rate of a vanishingly small packet at the holdings it was built from.
    fn price_pairs(&mut self) {
        let holdings = self.holdings();
        let mut prices = [None; MAX_PRICES];
        let mut slots = prices.iter_mut();
        for (from, entry_in) in self.ledgers().enumerate() {
            for (to, entry_out) in self.ledgers().enumerate() {
                if from == to {
                    continue;
                }
                let Ok(rate_e15) = self.rate_at(from, to, &holdings, 0, 0) else {
                    continue;
                };
                // There is a slot for every ordered pair.
                let Some(slot) = slots.next() else {
                    break;
                };
                *slot = Some(Price {
                    from: entry_in.ledger,
                    to: entry_out.ledger,
                    rate_e15,
                    as_of_ms: entry_in.as_of_ms.min(entry_out.as_of_ms),
                });
            }
        }
        self.prices = prices;
    }

    /// What the owner set for transit.
    #[must_use]
    pub const fn pricing(&self) -> Pricing {
        self.pricing
    }

    /// The charges on transit.
    #[must_use]
    pub const fn charges(&self) -> Charges {
        self.pricing.charges
    }

    /// When the table was built.
    #[must_use]
    pub const fn built_at_ms(&self) -> u64 {
        self.built_at_ms
    }

    /// The earliest time a rate the table uses ages out, or one it left
    /// out comes of age: build it again by then.
    #[must_use]
    pub const fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }

    /// How many assets its ledgers count toward
    /// ([`Self::build_by_asset`]): the `n` of each asset's target `1/n`.
    #[must_use]
    pub const fn assets(&self) -> usize {
        self.assets
    }

    /// The ledgers it prices.
    pub fn ledgers(&self) -> impl Iterator<Item = &LedgerPrice> + '_ {
        self.ledgers.iter().flatten()
    }

    /// Every price, by ledger pair: what a vanishingly small packet got
    /// when the table was built.
    pub fn prices(&self) -> impl Iterator<Item = &Price> + '_ {
        self.prices.iter().flatten()
    }

    /// Marks `ledger` as one that converts nothing now, such as a dollar
    /// token's while its peg guard pauses it
    /// (spec/decisions/issued-assets.md): no packet converts to or from
    /// it, not even at spot into the owner's own endpoints in the same
    /// currency, which needs no rate. The table is built without it.
    ///
    /// # Panics
    ///
    /// If the table prices `ledger`: one that converts nothing is left out
    /// of what it is built from.
    pub fn halt(&mut self, ledger: LedgerId) {
        assert!(
            self.ledger(ledger).is_none(),
            "a ledger that converts nothing is not priced"
        );
        if self.halted(ledger) {
            return;
        }
        // There is a slot for every ledger.
        if let Some(slot) = self.halted.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(ledger);
        }
    }

    /// Whether `ledger` converts nothing now ([`Self::halt`]).
    #[must_use]
    pub fn halted(&self, ledger: LedgerId) -> bool {
        self.halted.contains(&Some(ledger))
    }

    /// The ledger `ledger`, if it is priced.
    #[must_use]
    pub fn ledger(&self, ledger: LedgerId) -> Option<&LedgerPrice> {
        self.ledgers().find(|entry| entry.ledger == ledger)
    }

    /// The price from `from` to `to`, if the table has one.
    #[must_use]
    pub fn price(&self, from: LedgerId, to: LedgerId) -> Option<&Price> {
        self.prices()
            .find(|price| price.from == from && price.to == to)
    }

    /// What the owner held on each of its ledgers when it was built, in its
    /// order.
    #[must_use]
    pub fn holdings(&self) -> Holdings {
        let mut holdings = [Holding::NONE; MAX_PRICED_LEDGERS];
        for (slot, entry) in holdings.iter_mut().zip(self.ledgers()) {
            *slot = entry.holding;
        }
        holdings
    }

    /// What the owner holds on each of its ledgers now, by `ledger`'s books,
    /// in its order: one group's counters per ledger. A group the books do
    /// not have holds nothing.
    #[must_use]
    pub fn live_holdings(&self, ledger: &Ledger) -> Holdings {
        let mut holdings = [Holding::NONE; MAX_PRICED_LEDGERS];
        for (slot, entry) in holdings.iter_mut().zip(self.ledgers()) {
            *slot = ledger
                .group_balance(entry.owner)
                .map_or(Holding::NONE, |balance| Holding::of(&balance));
        }
        holdings
    }

    /// The rate, out per in times 10^15, that a packet from `from` to `to`
    /// gets which brings in `amount_in`, its fee included, and converts
    /// `net`, with the owner holding `holdings` on the table's ledgers: the
    /// curve at the worst state the packets in flight can leave, after the
    /// packet's own move, less the spread. Freshness is not checked.
    ///
    /// # Errors
    ///
    /// A ledger the table does not price ([`PriceError::Stale`]); a pair
    /// without a rate; or too little on `to`.
    pub fn rate(
        &self,
        from: LedgerId,
        to: LedgerId,
        amount_in: u64,
        net: u64,
        holdings: &Holdings,
    ) -> Result<u64, PriceError> {
        let index = |ledger| {
            self.ledgers()
                .position(|entry| entry.ledger == ledger)
                .ok_or(PriceError::Stale)
        };
        let (from, to) = (index(from)?, index(to)?);
        self.rate_at(from, to, holdings, u128::from(amount_in), u128::from(net))
    }

    /// What the owner's own curve counts a unit of `from` worth in units of
    /// `to`, out per in times 10^15, once `amount_in` came in on `from` and
    /// what `net` of it is worth at spot went out on `to`, with the owner
    /// holding `holdings`: the rate [`Self::rate`] gives a peer's packet
    /// from `from` to `to`, at the same worst state the packets in flight
    /// can leave and after the same move, but without the spread, which
    /// the node charges its peers and not itself. With `targets`, the curve
    /// aims for them in place of its equal targets: both assets need one,
    /// and the shares are of the assets that have one. A node's own swap is
    /// worth making while what it gets, valued so, clears what it pays and
    /// its costs (spec/decisions/peer-swaps.md). Freshness is not checked.
    ///
    /// # Errors
    ///
    /// As [`Self::rate`]; and [`PriceError::NoRate`] when `targets` leave
    /// out either asset.
    pub fn value_rate(
        &self,
        from: LedgerId,
        to: LedgerId,
        amount_in: u64,
        net: u64,
        holdings: &Holdings,
        targets: Option<&Targets>,
    ) -> Result<u64, PriceError> {
        let index = |ledger| {
            self.ledgers()
                .position(|entry| entry.ledger == ledger)
                .ok_or(PriceError::Stale)
        };
        let (from, to) = (index(from)?, index(to)?);
        let curve = Curve {
            spread_ppm: 0,
            targets,
        };
        let moved = (u128::from(amount_in), u128::from(net));
        self.rate_on(from, to, holdings, moved, curve)
    }

    /// [`Self::rate`], by the ledgers' places in the table.
    fn rate_at(
        &self,
        from: usize,
        to: usize,
        holdings: &Holdings,
        amount_in: u128,
        net: u128,
    ) -> Result<u64, PriceError> {
        let curve = Curve {
            spread_ppm: self.pricing.charges.spread_ppm,
            targets: None,
        };
        self.rate_on(from, to, holdings, (amount_in, net), curve)
    }

    /// The curve's rate from the ledger at `from` to the one at `to`, after
    /// `moved`, what came in and what of it goes out, on `curve`.
    fn rate_on(
        &self,
        from: usize,
        to: usize,
        holdings: &Holdings,
        moved: (u128, u128),
        curve: Curve<'_>,
    ) -> Result<u64, PriceError> {
        let (amount_in, net) = moved;
        let entries = &self.ledgers;
        let (Some(Some(entry_in)), Some(Some(entry_out))) = (entries.get(from), entries.get(to))
        else {
            return Err(PriceError::Stale);
        };
        if from == to {
            return Err(PriceError::NoRate);
        }
        assert!(u128::from(curve.spread_ppm) < MILLION, "a checked spread");
        // A pair whose rate at target, spot less the spread, rounds to
        // nothing or does not fit has no rate, whatever the owner holds.
        let spread = (MILLION - u128::from(curve.spread_ppm)) * 10u128.pow(12);
        if spot_times(entry_in, entry_out, spread).is_none_or(|at_target| at_target == 0) {
            return Err(PriceError::NoRate);
        }
        // What the packet takes out, at spot, in the common base.
        let taken = value_of(net, entry_in);
        let (asset_in, asset_out) = (entry_in.asset, entry_out.asset);
        // The owner's targets, if any: both assets need one, and only the
        // assets with one count toward the total.
        let counts = |asset| curve.counts(asset);
        if !(counts(asset_in) && counts(asset_out)) {
            return Err(PriceError::NoRate);
        }
        let mut total = 0u128;
        let (mut value_in, mut value_out, mut left_out) = (0u128, 0u128, 0u128);
        for (index, (entry, holding)) in entries.iter().flatten().zip(holdings).enumerate() {
            // Each ledger of the asset in at its most, each of the asset out
            // at its least, and the packet's own move on its two ledgers.
            let value = if index == from {
                value_of(holding.most().saturating_add(amount_in), entry)
            } else if index == to {
                left_out = value_of(holding.least(), entry).saturating_sub(taken);
                left_out
            } else if entry.asset == asset_in {
                value_of(holding.most(), entry)
            } else if entry.asset == asset_out {
                value_of(holding.least(), entry)
            } else {
                value_of(holding.posted, entry)
            };
            // At most 16 values of at most MAX_VALUE each, of which each
            // asset's are a part.
            if counts(entry.asset) {
                total = total.saturating_add(value);
            }
            if entry.asset == asset_in {
                value_in = value_in.saturating_add(value);
            } else if entry.asset == asset_out {
                value_out = value_out.saturating_add(value);
            }
        }
        // Out of a ledger with nothing left on it, nothing converts, however
        // much of its asset the owner holds on another: it cannot pay out.
        if left_out == 0 {
            return Err(PriceError::NoFunds);
        }
        // Between two ledgers of one asset the curve is one: spot, less the
        // spread.
        let multiplier = if asset_in == asset_out {
            spread
        } else {
            let (numerator, denominator) = factor(
                share(value_in, total),
                share(value_out, total),
                self.aims(curve.targets, asset_in, asset_out),
                self.pricing,
            )
            .ok_or(PriceError::NoFunds)?;
            mul_div(numerator, spread, denominator).ok_or(PriceError::NoRate)?
        };
        // At most `1 + k` times the rate at target: one that does not fit is
        // no rate, and one the penalty rounds to nothing is too little on
        // the way out.
        match spot_times(entry_in, entry_out, multiplier) {
            None => Err(PriceError::NoRate),
            Some(0) => Err(PriceError::NoFunds),
            Some(rate_e15) => Ok(rate_e15),
        }
    }

    /// What `net`, the incoming `amount_in` less the fee, converts to on
    /// transit from `from` to `to`, at `now_ms`, with the owner holding
    /// `holdings` ([`Self::live_holdings`]): `floor(net × rate / 10^15)` at
    /// [`Self::rate`].
    ///
    /// # Errors
    ///
    /// A ledger without a price, or one whose rate is older than
    /// `max_rate_age_ms`; a pair without a rate; too little on `to`; or a
    /// result that does not fit.
    #[allow(clippy::too_many_arguments)]
    pub fn convert(
        &self,
        from: LedgerId,
        to: LedgerId,
        amount_in: u64,
        net: u64,
        holdings: &Holdings,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Result<u64, PriceError> {
        let priced = |ledger| {
            self.ledger(ledger)
                .is_some_and(|entry| fresh(entry.as_of_ms, now_ms, max_rate_age_ms))
        };
        if !(priced(from) && priced(to)) {
            return Err(PriceError::Stale);
        }
        let rate_e15 = self.rate(from, to, amount_in, net, holdings)?;
        convert_at_rate(net, rate_e15).ok_or(PriceError::TooLarge)
    }

    /// What `amount` on `from` is worth on `to` at spot, uncharged: what a
    /// peer's packet into the owner's own endpoints converts at. The same
    /// currency converts one to one without a rate, unless either ledger
    /// converts nothing now ([`Self::halt`]).
    ///
    /// # Errors
    ///
    /// A rate it needs is missing or older than `max_rate_age_ms`, either
    /// ledger converts nothing now, or the result does not fit.
    pub fn convert_at_spot(
        &self,
        from: LedgerId,
        to: LedgerId,
        amount: u64,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Result<u64, PriceError> {
        if self.halted(from) || self.halted(to) {
            return Err(PriceError::Stale);
        }
        let (from_asset, to_asset) = (from.asset(), to.asset());
        let (from_rate, to_rate) = if from_asset.code == to_asset.code {
            (1, 1)
        } else {
            let rate = |ledger| {
                self.ledger(ledger)
                    .filter(|entry| fresh(entry.as_of_ms, now_ms, max_rate_age_ms))
                    .map(|entry| entry.rate)
                    .ok_or(PriceError::Stale)
            };
            (rate(from)?, rate(to)?)
        };
        crate::fx::convert_at(from_rate, to_rate, from_asset, to_asset, amount)
            .ok_or(PriceError::TooLarge)
    }

    /// What the curve aims for on a pair: its own targets, `1/n` each; or
    /// the targets of `asset_in` and `asset_out` among the owner's, as
    /// shares of their sum over the assets the table prices, rounded down.
    fn aims(&self, targets: Option<&Targets>, asset_in: LedgerId, asset_out: LedgerId) -> Aims {
        let Some(targets) = targets else {
            return Aims {
                target_in: SHARE_ONE,
                target_out: SHARE_ONE,
                per: u128::try_from(self.assets).unwrap_or(u128::MAX),
            };
        };
        let sum: u128 = targets
            .iter()
            .filter(|(asset, _)| self.ledgers().any(|entry| entry.asset == *asset))
            .map(|(_, ppm)| u128::from(ppm))
            .sum();
        let aim = |asset| {
            let ppm = u128::from(targets.get(asset).unwrap_or(0));
            mul_div(ppm, SHARE_ONE, sum).map_or(0, |share| share.min(SHARE_ONE))
        };
        Aims {
            target_in: aim(asset_in),
            target_out: aim(asset_out),
            per: 1,
        }
    }

    /// Of `candidates`, the ledger a packet from `from` goes out on, and
    /// its rate there: for a packet that brings in `amount_in`, its fee
    /// included, and converts `net`, with the owner holding `holdings`
    /// ([`Self::rate`]), at `now_ms`. `None` if no candidate has a rate:
    /// [`Self::convert`] says why for each.
    ///
    /// The packet keeps its ledger where it can
    /// (spec/decisions/settlement-methods.md): with `from` among the
    /// candidates, it is the answer, at one unit out per unit in, with no
    /// comparison and no rate needed, however much the node holds of
    /// another. Only otherwise does the packet convert, on the candidate
    /// where it gets the most relative to spot.
    ///
    /// Rates are out per in, in each output ledger's own units, so they are
    /// compared by value: `rate(from, b)` per what a unit of `b` is worth at
    /// spot, which ranks the candidates as `rate(from, b) · spot(b) /
    /// spot(from)` does, with `spot` the value of a unit. The comparison is
    /// exact, through 256-bit products, and a tie goes to the ledger first
    /// in [`LedgerId`]'s order, whatever the order of `candidates`. Each
    /// candidate needs a fresh rate on both ledgers, as [`Self::convert`]
    /// does. A candidate named twice counts once.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn best_output(
        &self,
        from: LedgerId,
        candidates: &[LedgerId],
        amount_in: u64,
        net: u64,
        holdings: &Holdings,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Option<BestOutput> {
        // What a unit of a ledger is worth at spot, as `value_of` divides
        // by: a ledger's spot rate at its scale, if fresh.
        let unit = |ledger| {
            self.ledger(ledger)
                .filter(|entry| fresh(entry.as_of_ms, now_ms, max_rate_age_ms))
                .and_then(|entry| value_unit(entry.rate, entry.ledger.asset().scale))
        };
        if candidates.contains(&from) {
            return Some(BestOutput {
                ledger: from,
                rate_e15: RATE_ONE_E15,
            });
        }
        let unit_in = unit(from)?;
        let mut best: Option<(BestOutput, u128)> = None;
        for &to in candidates {
            assert!(to != from);
            let Some(unit_out) = unit(to) else {
                continue;
            };
            let Ok(rate_e15) = self.rate(from, to, amount_in, net, holdings) else {
                continue;
            };
            assert!(unit_in > 0 && unit_out > 0);
            let candidate = (
                BestOutput {
                    ledger: to,
                    rate_e15,
                },
                unit_out,
            );
            best = Some(match best {
                Some(current) if !better(candidate, current) => current,
                _ => candidate,
            });
        }
        let (best, _) = best?;
        assert!(best.rate_e15 > 0, "a rate is never zero");
        assert!(candidates.contains(&best.ledger));
        Some(best)
    }
}

/// Whether `a`, a rate out per in and what a unit out is worth, gives more
/// relative to spot than `b` for the same packet in: `rate_a / unit_a >
/// rate_b / unit_b`, compared exactly as `rate_a · unit_b > rate_b ·
/// unit_a` in 256 bits. On a tie, the ledger first in order.
fn better(a: (BestOutput, u128), b: (BestOutput, u128)) -> bool {
    let ((output_a, unit_a), (output_b, unit_b)) = (a, b);
    let left = wide_mul(u128::from(output_a.rate_e15), unit_b);
    let right = wide_mul(u128::from(output_b.rate_e15), unit_a);
    match left.cmp(&right) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => output_a.ledger < output_b.ledger,
    }
}

/// `floor(net × rate_e15 / 10^15)`, if it fits.
fn convert_at_rate(net: u64, rate_e15: u64) -> Option<u64> {
    let out = u128::from(net) * u128::from(rate_e15) / u128::from(RATE_ONE_E15);
    u64::try_from(out).ok()
}

/// Whether a rate learned at `as_of_ms` is at most `max_age_ms` old at
/// `now_ms`, and not from after it.
pub(crate) const fn fresh(as_of_ms: u64, now_ms: u64, max_age_ms: u64) -> bool {
    as_of_ms <= now_ms && now_ms - as_of_ms <= max_age_ms
}

/// What one unit of an asset at `scale` divides into, at `rate`: the
/// denominator of [`value_of`]. `None` for a rate too large for its scale.
pub(crate) fn value_unit(rate: u128, scale: u8) -> Option<u128> {
    rate.checked_mul(10u128.checked_pow(u32::from(scale))?)
        .filter(|&unit| unit > 0)
}

/// What `amount` units of `entry`'s asset are worth in the common base, at
/// scale 9 and the base's rate of 10^[`RATE_PRECISION`], rounded down, at
/// most [`MAX_VALUE`]: the same amount is always worth the same, and more
/// never less.
pub(crate) fn value_of(amount: u128, entry: &LedgerPrice) -> u128 {
    let Some(unit) = value_unit(entry.rate, entry.ledger.asset().scale) else {
        return 0;
    };
    mul_div(amount, VALUE_BASE, unit).map_or(MAX_VALUE, |value| value.min(MAX_VALUE))
}

/// `value`'s share of `total`, in [`SHARE_ONE`]s, rounded down: it only
/// grows with `value`, and only shrinks with the rest of `total`. Nothing of
/// nothing.
fn share(value: u128, total: u128) -> u128 {
    mul_div(value, SHARE_ONE, total).map_or(0, |share| share.min(SHARE_ONE))
}

/// What the curve aims for on a pair: each asset's target, in shares, per
/// `per`. The curve's own targets are `1/n` for `n` assets, a whole share
/// per `n`; an owner's are shares of their own, per one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Aims {
    target_in: u128,
    target_out: u128,
    per: u128,
}

/// What a rate is taken on: the spread, and the owner's targets in place
/// of the curve's own.
#[derive(Clone, Copy, Debug)]
struct Curve<'a> {
    spread_ppm: u32,
    targets: Option<&'a Targets>,
}

impl Curve<'_> {
    /// Whether `asset` counts toward the total: every asset on the
    /// curve's own targets, only those with one on the owner's.
    fn counts(&self, asset: LedgerId) -> bool {
        self.targets
            .is_none_or(|targets| targets.get(asset).is_some())
    }
}

/// The curve's `w_in / w_out`, as a fraction, capped at `1 + k`; `None`
/// when the owner holds nothing on the way out. The two assets hold
/// `share_in` and `share_out`, and aim for `aims`.
fn factor(share_in: u128, share_out: u128, aims: Aims, pricing: Pricing) -> Option<(u128, u128)> {
    if share_out == 0 {
        return None;
    }
    let k = u128::from(pricing.rebalance_ppm);
    let cap = (MILLION + k, MILLION);
    if share_in == 0 {
        return Some(cap);
    }
    let Aims {
        target_in,
        target_out,
        per,
    } = aims;
    assert!(share_in <= SHARE_ONE && share_out <= SHARE_ONE && per <= 16);
    assert!(target_in <= SHARE_ONE && target_out <= SHARE_ONE);
    // `10^6 · p · s · w = (10^6 − k) · p · s + k · t`, for an asset of share
    // `s` and target `t / p`, in shares.
    let weight = |share: u128, target: u128| (MILLION - k) * per * share + k * target;
    let numerator = weight(share_in, target_in) * share_out;
    let denominator = share_in * weight(share_out, target_out);
    if numerator * MILLION > denominator * (MILLION + k) {
        Some(cap)
    } else {
        Some((numerator, denominator))
    }
}

/// Spot from `from` to `to`, in units out per unit in, times
/// `multiplier / 10^18`, times 10^15, rounded down: a price's `rate_e15`,
/// if it fits.
fn spot_times(from: &LedgerPrice, to: &LedgerPrice, multiplier: u128) -> Option<u64> {
    let (scale_in, scale_out) = (from.ledger.asset().scale, to.ledger.asset().scale);
    // A unit in is worth `rate_out / rate_in · 10^(scale_out − scale_in)`
    // units out; 10^18 for the multiplier over 10^15 for the rate leaves
    // 10^3 below.
    let (numerator, denominator) = if scale_out >= scale_in {
        let up = 10u128.checked_pow(u32::from(scale_out - scale_in))?;
        (to.rate.checked_mul(up)?, from.rate.checked_mul(1_000)?)
    } else {
        let down = 10u128.checked_pow(u32::from(scale_in - scale_out) + 3)?;
        (to.rate, from.rate.checked_mul(down)?)
    };
    u64::try_from(mul_div(numerator, multiplier, denominator)?).ok()
}

/// The low 64 bits.
const LOW: u128 = (1 << 64) - 1;

/// `floor(a × b / c)`, exact through a 256-bit product; `None` when `c` is
/// zero or the quotient does not fit 128 bits.
#[must_use]
pub(crate) fn mul_div(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    if let Some(product) = a.checked_mul(b) {
        return Some(product / c);
    }
    let (high, low) = wide_mul(a, b);
    if high >= c {
        return None;
    }
    // Long division a bit at a time; the remainder stays below `c`, and a
    // bit shifted out of it means it passed `c`.
    let mut remainder = high;
    let mut quotient = 0u128;
    for bit in (0..u128::BITS).rev() {
        let carry = remainder >> 127;
        remainder = (remainder << 1) | ((low >> bit) & 1);
        quotient <<= 1;
        if carry == 1 || remainder >= c {
            remainder = remainder.wrapping_sub(c);
            quotient |= 1;
        }
    }
    Some(quotient)
}

/// `a × b` as its high and low 128 bits.
pub(crate) const fn wide_mul(a: u128, b: u128) -> (u128, u128) {
    let (a_high, a_low) = (a >> 64, a & LOW);
    let (b_high, b_low) = (b >> 64, b & LOW);
    let low_low = a_low * b_low;
    let low_high = a_low * b_high;
    let high_low = a_high * b_low;
    let high_high = a_high * b_high;
    // At most three times 2^64: no overflow.
    let middle = (low_low >> 64) + (low_high & LOW) + (high_low & LOW);
    let low = (low_low & LOW) | ((middle & LOW) << 64);
    let high = high_high + (low_high >> 64) + (high_low >> 64) + (middle >> 64);
    (high, low)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    const USD: LedgerId = LedgerId::StubUsd;
    const XRP: LedgerId = LedgerId::XrplTestnetXrp;
    const RLUSD: LedgerId = LedgerId::XrplTestnetRlusdUsd;
    /// XRP in a settlement hub: a ledger of its own beside the XRP Ledger's.
    const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
    const DOLLAR: u64 = 1_000_000_000;
    const NOW: u64 = 1_000_000;
    const AGE: u64 = 3_600_000;

    fn rates() -> Rates {
        let mut rates = Rates::new();
        rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
        rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
        rates
    }

    fn pricing(k: u32, spread_ppm: u32) -> Pricing {
        Pricing {
            charges: Charges {
                fee_ppm: 0,
                spread_ppm,
            },
            rebalance_ppm: k,
        }
    }

    /// `ledger`, with `posted` and nothing in flight.
    fn held(ledger: LedgerId, posted: u64) -> Inventory {
        Inventory {
            ledger,
            owner: AccountId(0),
            holding: Holding {
                posted: u128::from(posted),
                ..Holding::NONE
            },
        }
    }

    fn build(inventories: &[Inventory], pricing: Pricing) -> PriceTable {
        PriceTable::build(&rates(), inventories, pricing, NOW, AGE)
    }

    fn rate(table: &PriceTable, from: LedgerId, to: LedgerId) -> u64 {
        table.price(from, to).unwrap().rate_e15
    }

    #[test]
    fn at_target_the_rate_is_spot_less_the_spread() {
        // 100 dollars, and 200 XRP worth 100 dollars.
        let inventories = [held(USD, 100 * DOLLAR), held(XRP, 200 * DOLLAR)];
        let table = build(&inventories, pricing(50_000, 0));
        assert_eq!(rate(&table, USD, XRP), 2 * RATE_ONE_E15);
        assert_eq!(rate(&table, XRP, USD), RATE_ONE_E15 / 2);
        let spread = build(&inventories, pricing(50_000, 5_000));
        assert_eq!(rate(&spread, USD, XRP), 1_990_000_000_000_000);
        assert_eq!(table.ledgers().count(), 2);
        assert_eq!(table.prices().count(), 2);
    }

    // The plan's worked examples, k = 5% and two ledgers: 25/75 costs about
    // 6%, 5/95 about 33% on the way out of the scarce ledger.
    #[test]
    fn a_skew_moves_the_rate_by_the_curve() {
        let skewed = |usd: u64, xrp_dollars: u64| {
            let inventories = [held(USD, usd * DOLLAR), held(XRP, 2 * xrp_dollars * DOLLAR)];
            build(&inventories, pricing(50_000, 0))
        };
        let quarter = skewed(25, 75);
        // w_USD = 0.95 + 0.05 · 2 = 1.05, w_XRP = 0.95 + 0.05 · 2/3: XRP to
        // dollars gives 59/63 of spot, 6.3% less.
        assert_eq!(rate(&quarter, XRP, USD), 468_253_968_253_968);
        // Dollars in get the capped bonus.
        assert_eq!(rate(&quarter, USD, XRP), 2_100_000_000_000_000);
        let twentieth = skewed(5, 95);
        let out = u128::from(rate(&twentieth, XRP, USD));
        // w_USD = 1.45, w_XRP ≈ 0.9763: about 67.3% of spot.
        assert!(out * 1_000 / u128::from(RATE_ONE_E15 / 2) == 673, "{out}");
    }

    // spec/decisions/passive-rebalancing.md: a packet pays its own impact.
    // At target, a tenth of the node's value in is priced at 60/40, where it
    // leaves the node: w_in = 0.95 + 0.05 · 0.5/0.6, w_out = 0.95 + 0.05 ·
    // 0.5/0.4.
    #[test]
    fn a_packet_pays_its_own_impact() {
        let inventories = [held(USD, 100 * DOLLAR), held(XRP, 200 * DOLLAR)];
        let table = build(&inventories, pricing(50_000, 0));
        let holdings = table.holdings();
        let tenth = 20 * DOLLAR;
        let rate_e15 = table.rate(USD, XRP, tenth, tenth, &holdings).unwrap();
        // 2 · 0.991667 / 1.0125: about 2% less than spot.
        assert_eq!(rate_e15 / 1_000_000_000_000, 1_958);
        assert!(rate_e15 < rate(&table, USD, XRP));
        // Nothing at all gets the table's rate, exactly.
        assert_eq!(
            table.rate(USD, XRP, 0, 0, &holdings),
            Ok(rate(&table, USD, XRP))
        );
        // What the packet converts to is at that rate.
        let out = table.convert(USD, XRP, tenth, tenth, &holdings, NOW, AGE);
        assert_eq!(
            out,
            Ok(u64::try_from(u128::from(tenth) * u128::from(rate_e15) / 10u128.pow(15)).unwrap())
        );
        // More than the node holds at spot on the way out has no price.
        assert_eq!(
            table.rate(USD, XRP, 100 * DOLLAR, 100 * DOLLAR, &holdings),
            Err(PriceError::NoFunds)
        );
    }

    // Packets in flight count against a packet: what comes in on its way in
    // as if fulfilled, and what goes out on its way out, as if fulfilled.
    // Their other halves do not count for it.
    #[test]
    fn packets_in_flight_count_against_a_packet() {
        let inventories = [held(USD, 100 * DOLLAR), held(XRP, 200 * DOLLAR)];
        let table = build(&inventories, pricing(50_000, 0));
        let at_target = table.holdings();
        let with = |index: usize, holding: Holding| {
            let mut holdings = at_target;
            holdings[index] = holding;
            table.rate(USD, XRP, 0, 0, &holdings).unwrap()
        };
        let spot = rate(&table, USD, XRP);
        let (usd, xrp) = (at_target[0], at_target[1]);
        let dollars = |count: u64| u128::from(count * DOLLAR);
        // Dollars on their way in, or XRP on its way out, make it worse.
        let usd_in = Holding {
            credits_pending: dollars(10),
            ..usd
        };
        let xrp_out = Holding {
            debits_pending: dollars(20),
            ..xrp
        };
        assert!(with(0, usd_in) < spot);
        assert!(with(1, xrp_out) < spot);
        // Dollars on their way out, or XRP on its way in, change nothing.
        let usd_out = Holding {
            debits_pending: dollars(50),
            ..usd
        };
        let xrp_in = Holding {
            credits_pending: dollars(100),
            ..xrp
        };
        assert_eq!(with(0, usd_out), spot);
        assert_eq!(with(1, xrp_in), spot);
        // The table is built at its holdings' worst state too.
        let mut pending = inventories;
        pending[0].holding = usd_in;
        let built = build(&pending, pricing(50_000, 0));
        assert!(rate(&built, USD, XRP) < spot);
        assert_eq!(rate(&built, XRP, USD), rate(&table, XRP, USD));
    }

    #[test]
    fn nothing_converts_out_of_a_ledger_with_nothing() {
        let inventories = [held(USD, 0), held(XRP, 2 * DOLLAR)];
        let table = build(&inventories, pricing(50_000, 0));
        let holdings = table.holdings();
        assert!(table.price(XRP, USD).is_none());
        assert_eq!(
            table.convert(XRP, USD, 1_000, 1_000, &holdings, NOW, AGE),
            Err(PriceError::NoFunds)
        );
        assert_eq!(rate(&table, USD, XRP), 2_100_000_000_000_000);
        // Once the rates are stale, that is what the packet hears.
        assert_eq!(
            table.convert(XRP, USD, 1_000, 1_000, &holdings, NOW + AGE + 1, AGE),
            Err(PriceError::Stale)
        );
        // With nothing anywhere, nothing converts either way.
        let empty = build(&[held(USD, 0), held(XRP, 0)], pricing(0, 0));
        assert_eq!(empty.ledgers().count(), 2);
        assert_eq!(empty.prices().count(), 0);
    }

    // A pair whose spot rate rounds to nothing or does not fit has no rate,
    // whatever the owner holds: as a missing rate, not a want of funds.
    #[test]
    fn a_rate_that_does_not_fit_is_no_rate() {
        // An XRP worth 10^18 dollars: a dollar unit buys less than 10^-15
        // drops, and a drop more than 10^19 dollar units.
        let mut rates = rates();
        rates.set_at("USD", 10u128.pow(18), NOW);
        rates.set_at("XRP", 1, NOW);
        let inventories = [held(USD, DOLLAR), held(XRP, DOLLAR)];
        let table = PriceTable::build(&rates, &inventories, pricing(0, 0), NOW, AGE);
        let holdings = table.holdings();
        assert!(table.price(USD, XRP).is_none());
        assert!(table.price(XRP, USD).is_none());
        for (from, to) in [(USD, XRP), (XRP, USD), (USD, USD)] {
            assert_eq!(
                table.rate(from, to, 1, 1, &holdings),
                Err(PriceError::NoRate)
            );
        }
    }

    #[test]
    fn without_k_the_rate_is_spot() {
        let inventories = [held(USD, DOLLAR), held(XRP, 1_000 * DOLLAR)];
        let table = build(&inventories, pricing(0, 0));
        assert_eq!(rate(&table, USD, XRP), 2 * RATE_ONE_E15);
        assert_eq!(rate(&table, XRP, USD), RATE_ONE_E15 / 2);
    }

    #[test]
    fn stale_and_missing_rates_leave_their_ledger_out() {
        let mut rates = rates();
        let inventories = [held(USD, DOLLAR), held(XRP, DOLLAR), held(RLUSD, DOLLAR)];
        let table = PriceTable::build(&rates, &inventories, pricing(50_000, 0), NOW + AGE, AGE);
        assert_eq!(table.ledgers().count(), 3);
        assert_eq!(table.prices().count(), 6);
        assert_eq!(table.valid_until_ms(), NOW + AGE + 1);
        let late = PriceTable::build(&rates, &inventories, pricing(50_000, 0), NOW + AGE + 1, AGE);
        assert_eq!(late.prices().count(), 0);
        // A rate from the future is left out until its time.
        rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW + 10);
        let early = PriceTable::build(&rates, &inventories, pricing(50_000, 0), NOW, AGE);
        assert!(early.ledger(XRP).is_none());
        assert_eq!(early.valid_until_ms(), NOW + 10);
        // A packet checks the age at its own time.
        assert_eq!(
            table.convert(USD, XRP, 1, 1, &table.holdings(), NOW + AGE + 1, AGE),
            Err(PriceError::Stale)
        );
        // Dollars on two ledgers held alike convert one to one.
        assert_eq!(rate(&table, USD, RLUSD), RATE_ONE_E15);
        let only = PriceTable::build(&Rates::new(), &inventories, pricing(0, 0), NOW, AGE);
        assert!(only.price(USD, RLUSD).is_none());
    }

    #[test]
    fn spot_conversion_is_exact_and_uncharged() {
        let inventories = [held(USD, 0), held(XRP, 0)];
        let table = build(&inventories, pricing(50_000, 5_000));
        assert_eq!(table.convert_at_spot(USD, XRP, 1_000, NOW, AGE), Ok(2_000));
        assert_eq!(table.convert_at_spot(XRP, USD, 3, NOW, AGE), Ok(1));
        assert_eq!(
            table.convert_at_spot(USD, XRP, 1, NOW + AGE + 1, AGE),
            Err(PriceError::Stale)
        );
        // The same currency needs no rate.
        let mut none = PriceTable::without_rates(Pricing::NONE);
        assert_eq!(none.convert_at_spot(USD, RLUSD, 7, NOW, AGE), Ok(7));
        // Unless either ledger converts nothing now, as a dollar token's
        // off its peg: then not even the same currency converts.
        none.halt(RLUSD);
        none.halt(RLUSD);
        assert!(none.halted(RLUSD) && !none.halted(USD));
        for (from, to) in [(USD, RLUSD), (RLUSD, USD)] {
            assert_eq!(
                none.convert_at_spot(from, to, 7, NOW, AGE),
                Err(PriceError::Stale)
            );
        }
        let mut halted = build(&inventories, pricing(50_000, 5_000));
        halted.halt(RLUSD);
        assert_eq!(halted.convert_at_spot(USD, XRP, 1_000, NOW, AGE), Ok(2_000));
        assert_eq!(
            halted.convert_at_spot(XRP, RLUSD, 2, NOW, AGE),
            Err(PriceError::Stale)
        );
    }

    // A group's balance as a holding: posted, and pending as the outside
    // sees it; the most and the least it comes to.
    #[test]
    fn a_holding_spans_what_is_in_flight() {
        let holding = Holding::of(&Balance {
            debits_posted: 10,
            credits_posted: 110,
            debits_pending: 30,
            credits_pending: 5,
        });
        assert_eq!(holding.posted, 100);
        assert_eq!((holding.least(), holding.most()), (70, 105));
        let below = Holding::of(&Balance {
            debits_posted: 10,
            credits_posted: 5,
            debits_pending: 1,
            credits_pending: 0,
        });
        assert_eq!((below.posted, below.least(), below.most()), (0, 0, 0));
    }

    proptest::proptest! {
        // Against arbitrary precision: exact, rounded down, and `None` only
        // when the quotient does not fit.
        #[test]
        fn mul_div_matches_arbitrary_precision(a: u128, b: u128, c in 1u128..) {
            use num_bigint::BigUint;
            let exact = BigUint::from(a) * BigUint::from(b) / BigUint::from(c);
            let fits = u128::try_from(&exact).ok();
            proptest::prop_assert_eq!(mul_div(a, b, c), fits);
        }
    }

    #[test]
    fn mul_div_is_exact() {
        assert_eq!(mul_div(6, 7, 4), Some(10));
        assert_eq!(mul_div(u128::MAX, u128::MAX, u128::MAX), Some(u128::MAX));
        assert_eq!(mul_div(u128::MAX, 2, 3), Some(u128::MAX / 3 * 2));
        assert_eq!(mul_div(u128::MAX, 3, 2), None);
        assert_eq!(mul_div(u128::MAX, 4, 6), Some(u128::MAX / 3 * 2));
        assert_eq!(mul_div(u128::MAX, 2, 1), None);
        assert_eq!(mul_div(1, 1, 0), None);
        assert_eq!(wide_mul(u128::MAX, u128::MAX), (u128::MAX - 1, 1));
    }

    /// The curve's `w_in / w_out` for shares from `values`, capped.
    fn exact_factor(values: &[u128], from: usize, to: usize, k: u32) -> Option<(u128, u128)> {
        let total = values.iter().sum();
        let aims = Aims {
            target_in: SHARE_ONE,
            target_out: SHARE_ONE,
            per: u128::try_from(values.len()).unwrap(),
        };
        factor(
            share(values[from], total),
            share(values[to], total),
            aims,
            pricing(k, 0),
        )
    }

    // The owner's own valuation is the curve without the spread, after the
    // same move a peer's packet makes; with targets of the owner's, the
    // curve aims for them, among the assets that have one.
    #[test]
    fn the_owners_valuation_is_the_curve_without_the_spread() {
        let (usd, xrp) = (USD, XRP);
        let k = pricing(50_000, 2_000);
        // At target: spot, with no spread.
        let even = build(&[held(usd, 100 * DOLLAR), held(xrp, 200 * DOLLAR)], k);
        let at_spot = 500_000_000_000_000;
        let holdings = even.holdings();
        assert_eq!(
            even.value_rate(usd, xrp, 0, 0, &holdings, None),
            Ok(2 * RATE_ONE_E15)
        );
        assert_eq!(
            even.value_rate(xrp, usd, 0, 0, &holdings, None),
            Ok(at_spot)
        );
        assert!(even.rate(xrp, usd, 0, 0, &holdings).unwrap() < at_spot);
        // A move counts against it, as against a peer's packet.
        let tenth = 20 * DOLLAR;
        let moved = even.value_rate(xrp, usd, tenth, tenth, &holdings, None);
        assert!(moved.unwrap() < at_spot);
        // Short of dollars: dollars are worth more than spot.
        let skewed = build(&[held(usd, 50 * DOLLAR), held(xrp, 200 * DOLLAR)], k);
        let holdings = skewed.holdings();
        assert!(skewed.value_rate(usd, xrp, 0, 0, &holdings, None).unwrap() > 2 * RATE_ONE_E15);
        // An owner who aims for a third in dollars holds them at target.
        let third = Targets::new(&[(usd, 1), (xrp, 2)]).unwrap();
        assert_eq!(
            skewed.value_rate(usd, xrp, 0, 0, &holdings, Some(&third)),
            Ok(2 * RATE_ONE_E15)
        );
        // Without a target on one of the assets, there is no value.
        let one = Targets::new(&[(usd, 1)]).unwrap();
        assert_eq!(
            skewed.value_rate(usd, xrp, 0, 0, &holdings, Some(&one)),
            Err(PriceError::NoRate)
        );
    }

    #[test]
    fn targets_name_each_asset_once_with_a_share() {
        let (usd, xrp) = (USD, XRP);
        assert!(Targets::new(&[]).unwrap().is_empty());
        let targets = Targets::new(&[(usd, 400_000), (xrp, 600_000)]).unwrap();
        assert_eq!(targets.get(xrp), Some(600_000));
        assert_eq!(targets.get(LedgerId::XrplTestnetRlusdUsd), None);
        assert_eq!(Targets::new(&[(usd, 1), (usd, 2)]), None);
        assert_eq!(Targets::new(&[(usd, 0)]), None);
        assert_eq!(Targets::new(&[(usd, 1_000_001)]), None);
    }

    #[test]
    fn the_factor_is_one_at_target_and_capped() {
        let (num, den) = exact_factor(&[5, 5], 0, 1, 50_000).unwrap();
        assert_eq!(num, den);
        let (num, den) = exact_factor(&[3, 3, 3], 2, 0, 100_000).unwrap();
        assert_eq!(num, den);
        assert_eq!(
            exact_factor(&[0, 5], 0, 1, 50_000),
            Some((1_050_000, 1_000_000))
        );
        assert_eq!(
            exact_factor(&[1, 99], 0, 1, 50_000),
            Some((1_050_000, 1_000_000))
        );
        assert_eq!(exact_factor(&[1, 0], 0, 1, 50_000), None);
    }

    // A settlement hub's ledger is priced as any other with a currency:
    // against dollars as the XRP Ledger's XRP is, and beside that, at par
    // with it at target, each valued on its own.
    #[test]
    fn a_hub_ledger_is_priced_as_any_other() {
        let k = pricing(50_000, 0);
        let hub = build(&[held(USD, 100 * DOLLAR), held(HUB, 200 * DOLLAR)], k);
        let direct = build(&[held(USD, 100 * DOLLAR), held(XRP, 200 * DOLLAR)], k);
        assert_eq!(rate(&hub, USD, HUB), rate(&direct, USD, XRP));
        assert_eq!(rate(&hub, HUB, USD), rate(&direct, XRP, USD));
        // The curve moves it as it moves any other: 25/75, as above.
        let skewed = build(&[held(USD, 25 * DOLLAR), held(HUB, 150 * DOLLAR)], k);
        assert_eq!(rate(&skewed, HUB, USD), 468_253_968_253_968);
        // Beside the XRP Ledger's XRP, one to one at target.
        let both = [
            held(USD, 100 * DOLLAR),
            held(XRP, 200 * DOLLAR),
            held(HUB, 200 * DOLLAR),
        ];
        let both = build(&both, k);
        assert_eq!(rate(&both, XRP, HUB), RATE_ONE_E15);
        assert_eq!(both.prices().count(), 6);
        // A hub the owner holds too little in: XRP paid out in it costs
        // more, and XRP brought into it gets the bonus.
        let short = [
            held(USD, 100 * DOLLAR),
            held(XRP, 300 * DOLLAR),
            held(HUB, 100 * DOLLAR),
        ];
        let short = build(&short, k);
        assert!(rate(&short, XRP, HUB) < RATE_ONE_E15);
        assert_eq!(rate(&short, HUB, XRP), 1_050_000_000_000_000);
        // Without a rate for its currency, it is left out as any other.
        let mut dollars = Rates::new();
        dollars.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
        let hub = [held(USD, DOLLAR), held(HUB, DOLLAR)];
        let unrated = PriceTable::build(&dollars, &hub, k, NOW, AGE);
        assert!(unrated.ledger(HUB).is_none() && unrated.prices().count() == 0);
    }

    /// The asset a ledger counts toward, as a node names it: for a hub's
    /// ledger, the owner book that funds the hub.
    const fn book_of(ledger: LedgerId) -> LedgerId {
        match ledger {
            LedgerId::HubXrplTestnetXrp => LedgerId::XrplTestnetXrp,
            LedgerId::HubXrplTestnetDsusdUsd => LedgerId::XrplTestnetDsusdUsd,
            LedgerId::HubXrplXrp => LedgerId::XrplXrp,
            LedgerId::HubXrplRlusdUsd => LedgerId::XrplRlusdUsd,
            other => other,
        }
    }

    /// The table of `inventories`, each asset counted whole: the XRP hub's
    /// ledger with the XRP Ledger's XRP, the owner book that funds it.
    fn whole(inventories: &[Inventory], pricing: Pricing) -> PriceTable {
        PriceTable::build_by_asset(&rates(), inventories, book_of, pricing, NOW, AGE)
    }

    // spec/decisions/passive-rebalancing.md: a node holds an asset in its
    // hub and in the owner book that funds it, and moves its funds between
    // the two by itself. With most of its XRP in the book, counted whole it
    // holds XRP at its target, and converts at spot; priced apart, its hub
    // would read short, and pay the capped bonus for XRP brought in there.
    #[test]
    fn an_asset_counts_whole_across_its_hub_and_its_book() {
        let k = pricing(50_000, 0);
        let inventories = [
            held(USD, 100 * DOLLAR),
            held(XRP, 190 * DOLLAR),
            held(HUB, 10 * DOLLAR),
        ];
        let table = whole(&inventories, k);
        assert_eq!(table.assets(), 2);
        assert_eq!(table.ledger(HUB).unwrap().asset, XRP);
        assert_eq!(table.ledger(XRP).unwrap().asset, XRP);
        assert_eq!(table.ledger(USD).unwrap().asset, USD);
        assert_eq!(rate(&table, HUB, USD), RATE_ONE_E15 / 2);
        assert_eq!(rate(&table, USD, HUB), 2 * RATE_ONE_E15);
        let apart = build(&inventories, k);
        assert_eq!(apart.assets(), 3);
        assert_eq!(rate(&apart, HUB, USD), 525_000_000_000_000);
        assert!(rate(&apart, USD, HUB) < 2 * RATE_ONE_E15);
        // With none of it in the hub, XRP is still at its target: what comes
        // in on the hub converts at spot, and nothing goes out on it, which
        // has nothing to pay out.
        let none = [
            held(USD, 100 * DOLLAR),
            held(XRP, 200 * DOLLAR),
            held(HUB, 0),
        ];
        let table = whole(&none, k);
        let holdings = table.holdings();
        assert_eq!(rate(&table, HUB, USD), RATE_ONE_E15 / 2);
        assert_eq!(rate(&table, HUB, XRP), RATE_ONE_E15);
        assert!(table.price(USD, HUB).is_none() && table.price(XRP, HUB).is_none());
        assert_eq!(
            table.rate(USD, HUB, 1, 1, &holdings),
            Err(PriceError::NoFunds)
        );
        assert_eq!(rate(&build(&none, k), HUB, USD), 525_000_000_000_000);
        // Short of XRP as a whole, however it is spread, the curve moves
        // the same: 25/75, as above, with the XRP in the hub or the book.
        for (book, hub) in [(150, 0), (75, 75), (1, 149)] {
            let skewed = [
                held(USD, 25 * DOLLAR),
                held(XRP, book * DOLLAR),
                held(HUB, hub * DOLLAR),
            ];
            let skewed = whole(&skewed, k);
            assert_eq!(rate(&skewed, HUB, USD), 468_253_968_253_968);
        }
        // A hub whose book is not priced counts alone.
        let alone = whole(&[held(USD, 100 * DOLLAR), held(HUB, 200 * DOLLAR)], k);
        assert_eq!(alone.assets(), 2);
        assert_eq!(rate(&alone, HUB, USD), RATE_ONE_E15 / 2);
    }

    // With two assets at target, however each is spread over its ledgers,
    // every pair converts at spot less the spread: between the two ledgers
    // of one asset, one to one less the spread.
    #[test]
    fn two_assets_at_target_convert_at_spot_less_the_spread() {
        let k = pricing(50_000, 5_000);
        for (book, hub) in [(200, 0), (120, 80), (1, 199)] {
            let inventories = [
                held(USD, 100 * DOLLAR),
                held(XRP, book * DOLLAR),
                held(HUB, hub * DOLLAR),
            ];
            let table = whole(&inventories, k);
            assert_eq!(rate(&table, USD, XRP), 1_990_000_000_000_000);
            assert_eq!(rate(&table, XRP, USD), 497_500_000_000_000);
            if hub > 0 {
                assert_eq!(rate(&table, USD, HUB), 1_990_000_000_000_000);
                assert_eq!(rate(&table, XRP, HUB), 995_000_000_000_000);
            }
            assert_eq!(rate(&table, HUB, USD), 497_500_000_000_000);
            assert_eq!(rate(&table, HUB, XRP), 995_000_000_000_000);
        }
    }

    // spec/decisions/passive-rebalancing.md: the worst state in flight
    // holds across an asset's ledgers. What is in flight on the book counts
    // against a packet on the hub as it would on the hub: XRP on its way
    // in makes bringing XRP in worse, XRP on its way out makes taking it
    // out worse, and neither other half ever makes either better.
    #[test]
    fn what_is_in_flight_on_the_book_counts_against_a_packet_on_the_hub() {
        let inventories = [
            held(USD, 100 * DOLLAR),
            held(XRP, 150 * DOLLAR),
            held(HUB, 50 * DOLLAR),
        ];
        let table = whole(&inventories, pricing(50_000, 0));
        let at_target = table.holdings();
        let with = |from: LedgerId, to: LedgerId, book: Holding| {
            let mut holdings = at_target;
            holdings[1] = book;
            table.rate(from, to, 0, 0, &holdings).unwrap()
        };
        let book = at_target[1];
        let coming = Holding {
            credits_pending: u128::from(20 * DOLLAR),
            ..book
        };
        let going = Holding {
            debits_pending: u128::from(20 * DOLLAR),
            ..book
        };
        let (into, out_of) = (rate(&table, HUB, USD), rate(&table, USD, HUB));
        assert!(with(HUB, USD, coming) < into);
        assert_eq!(with(HUB, USD, going), into);
        assert!(with(USD, HUB, going) < out_of);
        assert_eq!(with(USD, HUB, coming), out_of);
        // So a packet in on the hub is never priced better by a deposit on
        // its way from the book, held out of it and not yet in the hub.
        let mut deposit = at_target;
        deposit[1].debits_pending = u128::from(40 * DOLLAR);
        deposit[2].credits_pending = u128::from(40 * DOLLAR);
        assert!(table.rate(HUB, USD, 0, 0, &deposit).unwrap() < into);
        assert!(table.rate(USD, HUB, 0, 0, &deposit).unwrap() < out_of);
    }

    // The bounds hold with every ledger of a realm priced, its hubs
    // counted with their books, at holdings as large as a value counts.
    #[test]
    fn the_bounds_hold_with_every_ledger_of_a_realm_by_asset() {
        for realm in [Realm::Test, Realm::Live] {
            let mut ledgers: Vec<Inventory> = LedgerId::ALL
                .into_iter()
                .filter(|ledger| ledger.realm() == realm)
                .map(|ledger| held(ledger, u64::MAX))
                .collect();
            for inventory in &mut ledgers {
                inventory.holding.posted = u128::MAX;
                inventory.holding.credits_pending = u128::MAX;
            }
            assert!(ledgers.len() <= MAX_PRICED_LEDGERS);
            let table = whole(&ledgers, pricing(MAX_REBALANCE_PPM, 0));
            let hubs = ledgers
                .iter()
                .filter(|inventory| book_of(inventory.ledger) != inventory.ledger)
                .count();
            assert_eq!(table.ledgers().count(), ledgers.len());
            assert_eq!(table.assets(), ledgers.len() - hubs);
            let holdings = table.holdings();
            for from in table.ledgers() {
                for to in table.ledgers() {
                    if from.ledger != to.ledger {
                        let _ = table.rate(from.ledger, to.ledger, u64::MAX, u64::MAX, &holdings);
                    }
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "an asset's ledgers hold one currency of one realm")]
    fn an_asset_is_one_currency() {
        let _ = PriceTable::build_by_asset(
            &rates(),
            &[held(USD, DOLLAR), held(HUB, DOLLAR)],
            |_| USD,
            pricing(0, 0),
            NOW,
            AGE,
        );
    }

    #[test]
    #[should_panic(expected = "a table prices the ledgers of one realm")]
    fn a_table_prices_one_realm() {
        let realms = [held(USD, DOLLAR), held(LedgerId::XrplXrp, DOLLAR)];
        let _ = build(&realms, pricing(0, 0));
    }

    #[test]
    fn a_table_prices_every_ledger_of_a_realm() {
        let test = LedgerId::ALL.map(|ledger| held(ledger, DOLLAR));
        let test: Vec<Inventory> = test
            .into_iter()
            .filter(|inventory| inventory.ledger.realm() == Realm::Test)
            .collect();
        assert!(test.len() <= MAX_PRICED_LEDGERS);
        let table = build(&test, pricing(50_000, 0));
        assert_eq!(table.ledgers().count(), test.len());
        assert!(table.ledger(HUB).is_some());
    }

    // Four ledgers, the node at target on dollars and RLUSD, with too much
    // of the XRP Ledger's XRP and too little in the hub.
    fn skewed_four(k: Pricing) -> PriceTable {
        let inventories = [
            held(USD, 100 * DOLLAR),
            held(XRP, 300 * DOLLAR),
            held(RLUSD, 100 * DOLLAR),
            held(HUB, 100 * DOLLAR),
        ];
        build(&inventories, k)
    }

    // spec/areas/connector.md#comparing-outputs
    #[test]
    fn the_best_output_is_the_best_rate_relative_to_spot() {
        let table = skewed_four(pricing(50_000, 0));
        let holdings = table.holdings();
        let best = |from, candidates: &[LedgerId]| {
            table
                .best_output(from, candidates, 0, 0, &holdings, NOW, AGE)
                .map(|best| best.ledger)
        };
        // Out of dollars, the XRP the node has too much of gives the most,
        // dollars at par next, and the hub's XRP, which it lacks, the least,
        // though its rate, in units of XRP, is the larger number.
        assert_eq!(best(USD, &[HUB, RLUSD, XRP]), Some(XRP));
        assert_eq!(best(USD, &[HUB, RLUSD]), Some(RLUSD));
        assert!(rate(&table, USD, HUB) > rate(&table, USD, RLUSD));
        assert_eq!(
            table.best_output(USD, &[XRP], 0, 0, &holdings, NOW, AGE),
            Some(BestOutput {
                ledger: XRP,
                rate_e15: rate(&table, USD, XRP),
            })
        );
        // A packet keeps its ledger where it can, with no comparison:
        // exactly spot, even beside an output the node has too much of.
        assert_eq!(
            table.best_output(USD, &[HUB, USD], 0, 0, &holdings, NOW, AGE),
            Some(BestOutput {
                ledger: USD,
                rate_e15: RATE_ONE_E15,
            })
        );
        assert_eq!(best(USD, &[USD, XRP]), Some(USD));
        assert_eq!(best(USD, &[XRP, USD]), Some(USD));
        assert_eq!(best(HUB, &[HUB, XRP]), Some(HUB));
        // In on the hub, which the node lacks, a conversion pays out the XRP
        // it has too much of.
        assert_eq!(best(HUB, &[RLUSD, XRP]), Some(XRP));
        // The packet's own impact counts: a large one gets less, and none
        // on a ledger it would take more out of than the owner holds.
        let large = 80 * DOLLAR;
        let out = table.best_output(USD, &[HUB, XRP], large, large, &holdings, NOW, AGE);
        assert_eq!(out.map(|best| best.ledger), Some(XRP));
        assert!(out.is_some_and(|best| best.rate_e15 < rate(&table, USD, XRP)));
        let hub = table.best_output(USD, &[HUB], large, large, &holdings, NOW, AGE);
        assert_eq!(hub, None);
        // Nothing among no candidates.
        assert_eq!(best(USD, &[]), None);
    }

    // Ties go to the ledger first in order, whatever the candidates' order.
    #[test]
    fn a_tie_goes_to_the_ledger_first_in_order() {
        let inventories = [
            held(USD, 100 * DOLLAR),
            held(XRP, 200 * DOLLAR),
            held(RLUSD, 100 * DOLLAR),
            held(HUB, 200 * DOLLAR),
        ];
        let table = build(&inventories, pricing(50_000, 5_000));
        let holdings = table.holdings();
        let best = |candidates: &[LedgerId]| {
            table
                .best_output(USD, candidates, 0, 0, &holdings, NOW, AGE)
                .map(|best| best.ledger)
        };
        for candidates in [[HUB, RLUSD, XRP], [XRP, RLUSD, HUB], [RLUSD, HUB, XRP]] {
            assert_eq!(best(&candidates), Some(XRP));
        }
        assert_eq!(best(&[HUB, RLUSD, HUB]), Some(RLUSD));
        assert_eq!(best(&[HUB, HUB]), Some(HUB));
    }

    #[test]
    fn a_stale_output_is_no_candidate() {
        let table = skewed_four(pricing(50_000, 0));
        let holdings = table.holdings();
        let late = NOW + AGE + 1;
        let best = |from, candidates: &[LedgerId], now| {
            table
                .best_output(from, candidates, 0, 0, &holdings, now, AGE)
                .map(|best| best.ledger)
        };
        assert_eq!(best(USD, &[XRP, USD], late), Some(USD));
        assert_eq!(best(USD, &[XRP, RLUSD], late), None);
        assert_eq!(best(USD, &[XRP, HUB], late), None);
        // A ledger the table does not price can only stay.
        let unpriced = LedgerId::XrplTestnetDsusdUsd;
        assert_eq!(best(unpriced, &[XRP, unpriced], NOW), Some(unpriced));
        assert_eq!(best(USD, &[unpriced], NOW), None);
    }

    proptest::proptest! {
        // Against `rate(from, b) · spot(b) / spot(from)` in arbitrary
        // precision, with `spot` the value of a unit: the best, the first
        // in ledger order on a tie, whatever the candidates' order.
        #[test]
        fn the_best_output_matches_arbitrary_precision(
            amounts in proptest::array::uniform4(0u64..1_000),
            xrp_rate in 1u128..1_000_000_000_000_000_000,
            from in 0usize..4,
            chosen in 1u8..16,
            k in 0u32..=MAX_REBALANCE_PPM,
            amount in 0u64..100_000_000_000,
        ) {
            use num_bigint::BigUint;
            let ledgers = [USD, XRP, RLUSD, HUB];
            let mut rates = Rates::new();
            rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
            rates.set_at("XRP", xrp_rate, NOW);
            let inventories = ledgers
                .iter()
                .zip(amounts)
                .map(|(&ledger, units)| held(ledger, units * DOLLAR))
                .collect::<Vec<_>>();
            let table = PriceTable::build(&rates, &inventories, pricing(k, 0), NOW, AGE);
            let holdings = table.holdings();
            let from = ledgers[from];
            let candidates: Vec<LedgerId> = ledgers
                .into_iter()
                .enumerate()
                .filter(|(index, _)| chosen & (1 << index) != 0)
                .map(|(_, ledger)| ledger)
                .collect();
            // Each candidate's rate relative to spot, as a fraction, with a
            // unit's worth at the ledger's own scale.
            let spot = |ledger: LedgerId| {
                table.ledger(ledger).map(|entry| {
                    BigUint::from(entry.rate)
                        * BigUint::from(10u32).pow(u32::from(ledger.asset().scale))
                })
            };
            let mut expected: Option<(LedgerId, BigUint, BigUint)> = None;
            // The packet keeps its ledger where it can, uncompared.
            let converting = if candidates.contains(&from) {
                expected = Some((from, BigUint::from(1u8), BigUint::from(1u8)));
                &[][..]
            } else {
                &candidates[..]
            };
            for &to in converting {
                let Ok(rate_e15) = table.rate(from, to, amount, amount, &holdings) else {
                    continue;
                };
                let (Some(spot_in), Some(spot_out)) = (spot(from), spot(to)) else {
                    continue;
                };
                let (numerator, denominator) =
                    (BigUint::from(rate_e15) * spot_in, BigUint::from(RATE_ONE_E15) * spot_out);
                let wins = expected.as_ref().is_none_or(|(ledger, n, d)| {
                    let (this, that) = (&numerator * d, n * &denominator);
                    this > that || (this == that && to < *ledger)
                });
                if wins {
                    expected = Some((to, numerator, denominator));
                }
            }
            let expected = expected.map(|(ledger, _, _)| ledger);
            let best = |candidates: &[LedgerId]| {
                table
                    .best_output(from, candidates, amount, amount, &holdings, NOW, AGE)
                    .map(|best| best.ledger)
            };
            proptest::prop_assert_eq!(best(&candidates), expected);
            let reversed: Vec<LedgerId> = candidates.iter().rev().copied().collect();
            proptest::prop_assert_eq!(best(&reversed), expected);
        }
    }
}
