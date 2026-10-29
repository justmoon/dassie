//! What the market maker does next, as a pure function
//! (spec/areas/rate-feed.md#market-maker,
//! spec/decisions/market-maker-inventory.md).
//!
//! The market maker's one node, pinned to every target, holds two assets:
//! XRP and an issued token, each in the settlement hub of its asset and in
//! the owner book that funds it. It values what it holds as a core node
//! does, by a curve around an even share of each, but flatter, at the
//! XRP Ledger's AMM pool's price ([`Valuation`]). A trade is a swap with
//! one target, from the hub's method of asset `a` to the hub's method of
//! `b`, which the node sends for its owner (`SwapWithPeer`); it is worth it
//! while what comes back, valued by the curve at the state the trades in
//! flight leave after it, less what selling back on the pool the part of
//! it that takes the market maker beyond even would cost, exceeds what it
//! pays, valued alike, by the margin. Every target whose quote clears gets
//! a trade of its own, as large as its ladders allow with the trade still
//! clearing, and it asks the node for no less per unit than what clears.
//! A rebalance takes the market maker back to even once its share of an
//! asset is beyond a threshold: one Payment from the node's XRPL account to
//! itself, which spends the asset it holds too much of and delivers the
//! other, through the pool. One that cannot go leaves the trades back
//! toward even running.
//!
//! Every amount is in the node's ledger units: all its books count at scale 9
//! (spec/areas/ledger.md#ledgers-and-amounts), so a drop is
//! [`UNITS_PER_DROP`] units and a millionth of the token [`UNITS_PER_MICRO`]
//! units. Converting to the ledger's amounts rounds down, so the market
//! maker never counts on more than it gets. Nothing passes through a float.

use std::sync::Arc;

use dassie_feed::{MethodTag, Snapshot, method_tag};
use dassie_identity::NodeId;
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::amm::{self, SwapError};
use dassie_xrpl::amount::{Amount, IssuedAsset, TokenAmount, TokenValue};

/// Ledger units per millionth of the token, as the token scheme books it.
pub const UNITS_PER_MICRO: u64 = dassie_xrpl::token::UNITS_PER_MICRO;

/// The decimals of a millionth.
const MICRO_DECIMALS: u8 = 6;

/// Parts per million.
pub const MILLION: u32 = 1_000_000;

/// The steepest curve a config may ask for: a tenth, as a core node's
/// `rebalance_ppm` at most.
pub const MAX_CURVE_PPM: u32 = 100_000;

/// The largest margin a config may ask for: half again what a trade costs.
pub const MAX_MARGIN_PPM: u32 = 500_000;

/// The farthest from even a config may let its share go before it
/// rebalances: 40 points, short of holding one asset alone.
pub const MAX_THRESHOLD_PPM: u32 = 400_000;

/// The largest slippage a rebalance may allow: a tenth.
pub const MAX_SLIPPAGE_PPM: u32 = 100_000;

/// The most trades at once: as many owner swaps as the node runs at once
/// (`MAX_OWNER_SWAPS` in `crates/dassie-node/src/engine/core/swapping.rs`).
pub const MAX_TRADES: usize = 4;

/// The most a rebalance pays the XRP Ledger in fees, in drops: ten times
/// its reference fee. Under a heavier load it waits.
pub const MAX_FEE_DROPS: u64 = 100;

/// What a rebalance that spends XRP leaves the account for the fee.
const FEE_HEADROOM_UNITS: u64 = MAX_FEE_DROPS * UNITS_PER_DROP;

/// The pool's trading fee's unit per part per million: it counts in
/// hundred-thousandths.
const FEE_PPM_PER_UNIT: u32 = 10;

/// How many halvings a search of an amount takes at most: one per bit.
const SEARCH_STEPS: u32 = 64;

/// Which of the node's two assets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// XRP.
    Xrp,
    /// The token.
    Token,
}

impl Side {
    /// Both, in the order decisions look at them.
    pub const BOTH: [Self; 2] = [Self::Xrp, Self::Token];

    /// The other asset.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Xrp => Self::Token,
            Self::Token => Self::Xrp,
        }
    }

    /// Its name in log lines.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Xrp => "xrp",
            Self::Token => "token",
        }
    }
}

/// The strategy's settings, from the config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// How steeply its valuation follows its inventory, `k`, in parts per
    /// million: flatter than a core node's.
    pub curve_ppm: u32,
    /// The least gain a trade must make after the cost of undoing it, in
    /// parts per million of what it pays.
    pub margin_ppm: u32,
    /// How far an asset's share of its value may be from even before it
    /// rebalances, in parts per million of its value.
    pub threshold_ppm: u32,
    /// How far the pool's price may move against a rebalance before it
    /// fails, in parts per million.
    pub slippage_ppm: u32,
    /// The most one trade pays of XRP, in its ledgers' units.
    pub xrp_cap: u64,
    /// The most one trade pays of the token, in its ledgers' units.
    pub token_cap: u64,
}

impl Params {
    /// The most one trade pays of `side`'s asset.
    #[must_use]
    pub const fn cap(&self, side: Side) -> u64 {
        match side {
            Side::Xrp => self.xrp_cap,
            Side::Token => self.token_cap,
        }
    }
}

/// What a target's snapshot says it gives between the node's two
/// methods, the hubs' (spec/areas/rate-feed.md#reading-a-snapshot).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quotes {
    snapshot: Arc<Snapshot>,
    /// The tag of the XRP hub's method.
    xrp: MethodTag,
    /// The tag of the token hub's method.
    token: MethodTag,
}

impl Quotes {
    /// What `snapshot` says between the methods whose schemes' IDs are
    /// `xrp` and `token`.
    #[must_use]
    pub fn new(snapshot: Arc<Snapshot>, xrp: &str, token: &str) -> Self {
        Self {
            snapshot,
            xrp: method_tag(xrp),
            token: method_tag(token),
        }
    }

    /// The tags of `side`'s method and the other's.
    const fn tags(&self, side: Side) -> (MethodTag, MethodTag) {
        match side {
            Side::Xrp => (self.xrp, self.token),
            Side::Token => (self.token, self.xrp),
        }
    }

    /// Whether the snapshot prices a swap from `side`'s method to the
    /// other's: it quotes both, and neither side is closed.
    #[must_use]
    pub fn prices(&self, side: Side) -> bool {
        let (from, to) = self.tags(side);
        self.snapshot.swap_out(from, to, 0).is_some()
    }

    /// The least `amount` of `side`'s method gets out on the other's, by
    /// the snapshot's ladders, sent as a packet per step; `None` beyond
    /// what they cover, or for a pair it does not price.
    #[must_use]
    pub fn out(&self, side: Side, amount: u64) -> Option<u64> {
        let (from, to) = self.tags(side);
        self.snapshot.swap_out(from, to, amount)
    }
}

/// The pool of XRP and the token, as `amm_info` read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool {
    /// Its XRP, in drops.
    pub drops: u64,
    /// Its token.
    pub value: TokenValue,
    /// The token.
    pub token: IssuedAsset,
    /// Its trading fee, in hundred-thousandths.
    pub trading_fee: u32,
}

impl Pool {
    /// What it holds of `side`'s asset.
    #[must_use]
    pub const fn holds(&self, side: Side) -> Amount {
        match side {
            Side::Xrp => Amount::Xrp(self.drops),
            Side::Token => Amount::Token(TokenAmount {
                value: self.value,
                currency: self.token.currency,
                issuer: self.token.issuer,
            }),
        }
    }
}

/// What the node holds of one asset, in its ledgers' units, none below
/// zero (spec/areas/rate-feed.md#holdings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holding {
    /// What the owner holds of the asset, by the node's books: its funds
    /// in the hub's ledger and in the owner book, short of a move between
    /// them on its way, never above. What the curve values.
    pub total: u64,
    /// What the owner book's XRPL account holds of it that the owner may
    /// spend now, as the books have it: what a rebalance can spend. Less
    /// than the total by what is in the hub, which is what a swap pays
    /// from.
    pub on_ledger: u64,
}

impl Holding {
    /// What is in the hub, which a swap pays from.
    #[must_use]
    pub const fn in_hub(&self) -> u64 {
        self.total.saturating_sub(self.on_ledger)
    }
}

/// What the node holds of each asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holdings {
    /// Of XRP.
    pub xrp: Holding,
    /// Of the token.
    pub token: Holding,
}

impl Holdings {
    /// `side`'s.
    #[must_use]
    pub const fn of(&self, side: Side) -> Holding {
        match side {
            Side::Xrp => self.xrp,
            Side::Token => self.token,
        }
    }
}

/// What one target offers: its quotes, from the snapshot of it the node
/// holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetQuote {
    /// Which target, by node ID; a trade through it names it.
    pub target: NodeId,
    /// Its quotes, from a current snapshot.
    pub quotes: Quotes,
}

/// Everything a decision looks at.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The targets with a current snapshot that the market maker may trade
    /// through now, the node's peers, each once: at most as many as the
    /// node reads snapshots of. A target whose last trade failed or paid
    /// nothing is left out for a while.
    pub targets: &'a [TargetQuote],
    /// The pool.
    pub pool: Pool,
    /// The node's holdings.
    pub holdings: Holdings,
    /// The settings.
    pub params: &'a Params,
    /// Whether it may rebalance now: not while a rebalance that failed
    /// waits to be tried again.
    pub may_rebalance: bool,
}

/// A trade: a swap with `target` of up to `debit` of `from`'s asset, from
/// its hub's method to the other asset's, at no less than `least` back for
/// all of it, per unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trade {
    /// The target it swaps with, as its [`TargetQuote`] names it.
    pub target: NodeId,
    /// The asset it pays.
    pub from: Side,
    /// The most it pays, in its ledgers' units.
    pub debit: u64,
    /// What the snapshot says comes back at least for all of it, in the
    /// other asset's units.
    pub receive: u64,
    /// The least that may come back for all of it and still clear: the
    /// node is asked for no less than `least / debit` per unit it pays, at
    /// most `receive`.
    pub least: u64,
}

impl Trade {
    /// The least the swap takes back per unit it pays, as `SwapWithPeer`
    /// names it: `least / debit`.
    #[must_use]
    pub const fn rate(&self) -> (u64, u64) {
        (self.least, self.debit)
    }
}

/// A rebalance: one Payment from the node's XRPL account to itself, which
/// spends `from`'s asset and delivers the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rebalance {
    /// The asset it spends.
    pub from: Side,
    /// What the Payment delivers, of the other asset.
    pub deliver: Amount,
    /// The most it spends, of `from`'s asset: what takes it back to even.
    pub send_max: Amount,
}

/// Why nothing is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// No target has a current snapshot.
    NoTarget,
    /// No snapshot prices a swap between the two methods.
    NoRate,
    /// Nothing to pay with: the hubs hold none of what a trade would pay.
    Funds,
    /// No trade clears the margin.
    Margin,
    /// A rebalance would deliver nothing.
    Dust,
    /// The pool cannot price it.
    Pool(SwapError),
    /// An amount does not fit the ledger's units.
    Range,
}

/// What to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing, and why.
    Hold(Hold),
    /// Swap through these targets, one trade each, at once: at most
    /// [`MAX_TRADES`].
    Trades(Vec<Trade>),
    /// Take the holdings back to even on the ledger.
    Rebalance(Rebalance),
}

/// What to do now: a rebalance if an asset's share is beyond the
/// threshold from even, else a trade through each target whose quote
/// clears. A rebalance that cannot go, or may not yet, leaves the trades
/// running, but only those back toward even: from the asset it holds too
/// much of.
#[must_use]
pub fn decide(inputs: &Inputs<'_>) -> Decision {
    let valuation = match Valuation::of(inputs) {
        Ok(valuation) => valuation,
        Err(hold) => return Decision::Hold(hold),
    };
    let over = valuation.beyond_threshold(inputs.params.threshold_ppm);
    let from: &[Side] = match over {
        None => &Side::BOTH,
        Some(side) => {
            if inputs.may_rebalance
                && let Ok(rebalance) = rebalance(inputs, &valuation, side)
            {
                return Decision::Rebalance(rebalance);
            }
            match side {
                Side::Xrp => &[Side::Xrp],
                Side::Token => &[Side::Token],
            }
        }
    };
    trades(inputs, &valuation, from)
}

/// The trades through the targets, from the assets `from`, or why there
/// are none. Each round takes, among the targets not yet given one, the
/// trade that gains most per unit valued, and counts it as done at what
/// its snapshot says before the next is chosen, so trades at once never
/// together take the market maker further than each assumed. Of all the
/// reasons met, the one a trade got furthest to is given.
fn trades(inputs: &Inputs<'_>, valuation: &Valuation, from: &[Side]) -> Decision {
    let mut state = valuation.clone();
    let mut chosen: Vec<Trade> = Vec::new();
    let mut hold = Hold::NoTarget;
    for _ in 0..MAX_TRADES {
        let mut best: Option<(Trade, u128, u128)> = None;
        for quote in inputs.targets {
            if chosen.iter().any(|trade| trade.target == quote.target) {
                continue;
            }
            for &side in from {
                match state.best_trade(inputs, quote, side) {
                    Ok(trade) => {
                        let (gain, paid) = state.gain(trade.from, trade.debit, trade.receive);
                        let better = best.is_none_or(|(_, best_gain, best_paid)| {
                            gain.saturating_mul(best_paid) > best_gain.saturating_mul(paid)
                        });
                        if better {
                            best = Some((trade, gain, paid));
                        }
                    }
                    Err(reason) => hold = further(hold, reason),
                }
            }
        }
        let Some((trade, _, _)) = best else { break };
        state = state.after(trade.from, trade.debit, trade.receive);
        chosen.push(trade);
    }
    if chosen.is_empty() {
        Decision::Hold(hold)
    } else {
        Decision::Trades(chosen)
    }
}

/// Of two reasons not to trade, the one a trade got further to: no
/// target, then no rate, then nothing to pay with, then an amount the
/// ledger or the pool cannot take, and last a trade priced that did not
/// clear. So the reason logged does not depend on the order of the
/// targets.
const fn further(a: Hold, b: Hold) -> Hold {
    const fn stage(hold: Hold) -> u8 {
        match hold {
            Hold::NoTarget | Hold::Dust => 0,
            Hold::NoRate => 1,
            Hold::Funds => 2,
            Hold::Range => 3,
            Hold::Pool(_) => 4,
            Hold::Margin => 5,
        }
    }
    if stage(b) > stage(a) { b } else { a }
}

/// What the market maker holds, and what is left to pay with in each hub,
/// valued at the pool's price in the token's units (spec/decisions/market-maker-inventory.md).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Valuation {
    /// What it holds of XRP and of the token, in their units.
    xrp: u64,
    token: u64,
    /// What is left in each hub to pay a trade with.
    xrp_in_hub: u64,
    token_in_hub: u64,
    /// The pool's XRP and token, in their units: its price.
    pool_xrp: u128,
    pool_token: u128,
    /// The curve's `k`, and what undoing a trade beyond even costs, for
    /// each asset that comes in, in parts per million of its value.
    curve_ppm: u128,
    undo_xrp_ppm: u128,
    undo_token_ppm: u128,
    margin_ppm: u128,
}

impl Valuation {
    /// The holdings and the pool's price `inputs` give, and what undoing a
    /// trade costs on the pool now.
    fn of(inputs: &Inputs<'_>) -> Result<Self, Hold> {
        let pool = inputs.pool;
        let pool_xrp = u128::from(to_units(pool.holds(Side::Xrp)).ok_or(Hold::Range)?);
        let pool_token = u128::from(to_units(pool.holds(Side::Token)).ok_or(Hold::Range)?);
        if pool_xrp == 0 || pool_token == 0 {
            return Err(Hold::Pool(SwapError::Dry));
        }
        let holdings = inputs.holdings;
        let mut valuation = Self {
            xrp: holdings.xrp.total,
            token: holdings.token.total,
            xrp_in_hub: holdings.xrp.in_hub(),
            token_in_hub: holdings.token.in_hub(),
            pool_xrp,
            pool_token,
            curve_ppm: u128::from(inputs.params.curve_ppm),
            undo_xrp_ppm: 0,
            undo_token_ppm: 0,
            margin_ppm: u128::from(inputs.params.margin_ppm),
        };
        valuation.undo_xrp_ppm = valuation.undo_ppm(inputs, Side::Xrp)?;
        valuation.undo_token_ppm = valuation.undo_ppm(inputs, Side::Token)?;
        Ok(valuation)
    }

    /// `units` of `side`'s asset, valued at the pool's price, in the
    /// token's units, rounded down.
    fn value(&self, side: Side, units: u64) -> u128 {
        match side {
            Side::Token => u128::from(units),
            Side::Xrp => u128::from(units).saturating_mul(self.pool_token) / self.pool_xrp,
        }
    }

    /// `value`, in the token's units, as units of `side`'s asset, rounded
    /// down.
    fn units_of(&self, side: Side, value: u128) -> Option<u64> {
        let units = match side {
            Side::Token => value,
            Side::Xrp => value.checked_mul(self.pool_xrp)? / self.pool_token,
        };
        u64::try_from(units).ok()
    }

    /// What it holds of `side`'s asset, and what is in its hub.
    const fn holds(&self, side: Side) -> (u64, u64) {
        match side {
            Side::Xrp => (self.xrp, self.xrp_in_hub),
            Side::Token => (self.token, self.token_in_hub),
        }
    }

    /// Its value of each asset, and the total.
    fn values(&self) -> (u128, u128, u128) {
        let xrp = self.value(Side::Xrp, self.xrp);
        let token = self.value(Side::Token, self.token);
        (xrp, token, xrp.saturating_add(token))
    }

    /// The asset whose share of its value is more than `threshold_ppm`
    /// from even, if one is.
    fn beyond_threshold(&self, threshold_ppm: u32) -> Option<Side> {
        let (xrp, token, total) = self.values();
        if total == 0 {
            return None;
        }
        let (over, under) = if xrp >= token {
            (Side::Xrp, xrp - token)
        } else {
            (Side::Token, token - xrp)
        };
        // A share `threshold` from even is one asset `2 · threshold` of
        // the total above the other.
        let limit = total.saturating_mul(2 * u128::from(threshold_ppm)) / u128::from(MILLION);
        (under > limit).then_some(over)
    }

    /// The curve's weight of `side`'s asset, in parts per million:
    /// `(1 − k) + k · ½ / s`, capped at `1 + k`, rounded `up` or down.
    fn weight(&self, side: Side, up: bool) -> u128 {
        let (xrp, token, total) = self.values();
        let held = match side {
            Side::Xrp => xrp,
            Side::Token => token,
        };
        let million = u128::from(MILLION);
        let most = million + self.curve_ppm;
        if held == 0 {
            return most;
        }
        let numerator = self.curve_ppm.saturating_mul(total);
        let denominator = held.saturating_mul(2);
        let bonus = if up {
            numerator.div_ceil(denominator)
        } else {
            numerator / denominator
        };
        (million - self.curve_ppm).saturating_add(bonus).min(most)
    }

    /// The state after paying `paid` of `from`'s asset and taking in
    /// `received` of the other.
    fn after(&self, from: Side, paid: u64, received: u64) -> Self {
        let mut next = self.clone();
        match from {
            Side::Xrp => {
                next.xrp = next.xrp.saturating_sub(paid);
                next.xrp_in_hub = next.xrp_in_hub.saturating_sub(paid);
                next.token = next.token.saturating_add(received);
            }
            Side::Token => {
                next.token = next.token.saturating_sub(paid);
                next.token_in_hub = next.token_in_hub.saturating_sub(paid);
                next.xrp = next.xrp.saturating_add(received);
            }
        }
        next
    }

    /// What selling back on the pool a rebalance's worth of `side`'s asset
    /// costs, in parts per million of its value: the pool's fee, and the
    /// price impact of a rebalance of the threshold's share of the
    /// market maker's value, rounded up; at least the fee.
    fn undo_ppm(&self, inputs: &Inputs<'_>, side: Side) -> Result<u128, Hold> {
        let fee = u128::from(inputs.pool.trading_fee) * u128::from(FEE_PPM_PER_UNIT);
        let (_, _, total) = self.values();
        let size_value =
            total.saturating_mul(u128::from(inputs.params.threshold_ppm)) / u128::from(MILLION);
        let size = self.units_of(side, size_value).ok_or(Hold::Range)?;
        let sold = self.value(side, size);
        if sold == 0 {
            return Ok(fee);
        }
        let back = swap_back(&inputs.pool, side, size)?;
        let back_value = self.value(side.other(), back);
        let lost = sold.saturating_sub(back_value);
        let ppm = lost.saturating_mul(u128::from(MILLION)).div_ceil(sold);
        Ok(ppm.max(fee))
    }

    /// What paying `paid` of `from`'s asset for `received` of the other
    /// gains, and what it pays, both valued by the curve at the state
    /// after it, in the token's units times a million squared: what comes
    /// back less the cost of undoing its part beyond even, against what is
    /// paid with the margin. The gain is zero if it does not clear.
    fn gain(&self, from: Side, paid: u64, received: u64) -> (u128, u128) {
        let to = from.other();
        let after = self.after(from, paid, received);
        let million = u128::from(MILLION);
        let paid_value = self.value(from, paid);
        let received_value = self.value(to, received);
        let (xrp, token, total) = after.values();
        let to_value = match to {
            Side::Xrp => xrp,
            Side::Token => token,
        };
        let beyond = received_value.min(to_value.saturating_sub(total / 2));
        let undo = match to {
            Side::Xrp => self.undo_xrp_ppm,
            Side::Token => self.undo_token_ppm,
        };
        let net = received_value
            .saturating_mul(million)
            .saturating_sub(beyond.saturating_mul(undo));
        let back = net.saturating_mul(after.weight(to, false));
        let cost = paid_value
            .saturating_mul(after.weight(from, true))
            .saturating_mul(million + self.margin_ppm);
        (back.saturating_sub(cost), cost.max(1))
    }

    /// Whether paying `paid` of `from`'s asset for `received` of the other
    /// clears.
    fn clears(&self, from: Side, paid: u64, received: u64) -> bool {
        paid > 0 && self.gain(from, paid, received).0 > 0
    }

    /// The trade from `from`'s asset through `quote`: as much as the
    /// target's ladders cover with the trade still clearing, up to the cap
    /// and what is left in the hub, at the least back that clears.
    fn best_trade(
        &self,
        inputs: &Inputs<'_>,
        quote: &TargetQuote,
        from: Side,
    ) -> Result<Trade, Hold> {
        if !quote.quotes.prices(from) {
            return Err(Hold::NoRate);
        }
        let (held, in_hub) = self.holds(from);
        let most = inputs.params.cap(from).min(held).min(in_hub);
        if most == 0 {
            return Err(Hold::Funds);
        }
        let clears_at = |paid: u64| {
            quote
                .quotes
                .out(from, paid)
                .is_some_and(|out| self.clears(from, paid, out))
        };
        let debit = largest(most, clears_at).ok_or(Hold::Margin)?;
        let receive = quote.quotes.out(from, debit).ok_or(Hold::Range)?;
        let least = smallest(receive, |back| self.clears(from, debit, back)).ok_or(Hold::Margin)?;
        Ok(Trade {
            target: quote.target,
            from,
            debit,
            receive,
            least,
        })
    }
}

/// The largest amount up to `most` for which `holds` does, if any does at
/// a small one: a sixty-fourth of `most`, or `most` itself. `holds` is
/// taken to hold for every amount below one it holds for.
fn largest(most: u64, holds: impl Fn(u64) -> bool) -> Option<u64> {
    if holds(most) {
        return Some(most);
    }
    let mut low = (most / 64).max(1);
    if low >= most || !holds(low) {
        return None;
    }
    let mut high = most;
    for _ in 0..SEARCH_STEPS {
        if high - low <= 1 {
            break;
        }
        let middle = low + (high - low) / 2;
        if holds(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(low)
}

/// The smallest amount up to `most` for which `holds` does, if it does at
/// `most`. `holds` is taken to hold for every amount above one it holds
/// for.
fn smallest(most: u64, holds: impl Fn(u64) -> bool) -> Option<u64> {
    if !holds(most) {
        return None;
    }
    let mut low = 0;
    let mut high = most;
    for _ in 0..SEARCH_STEPS {
        if high - low <= 1 {
            break;
        }
        let middle = low + (high - low) / 2;
        if holds(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    Some(high)
}

/// The rebalance of `over`'s asset, whose share is beyond the threshold:
/// it spends what takes the market maker back to even, at most what its
/// owner book's account holds now (less the fee's headroom, for XRP), and
/// asks the pool for what a slippage's share less buys, so the price may
/// move by that before it fails.
fn rebalance(inputs: &Inputs<'_>, valuation: &Valuation, over: Side) -> Result<Rebalance, Hold> {
    let (xrp, token, _) = valuation.values();
    let surplus_value = xrp.abs_diff(token) / 2;
    let surplus = valuation.units_of(over, surplus_value).ok_or(Hold::Range)?;
    let holding = inputs.holdings.of(over);
    let spendable = match over {
        Side::Xrp => holding.on_ledger.saturating_sub(FEE_HEADROOM_UNITS),
        Side::Token => holding.on_ledger,
    };
    let excess = surplus.min(spendable);
    let slippage = u128::from(MILLION) + u128::from(inputs.params.slippage_ppm);
    let target = u64::try_from(u128::from(excess) * u128::from(MILLION) / slippage)
        .map_err(|_| Hold::Range)?;
    let token_asset = inputs.pool.token;
    let send_max = to_amount(over, excess, token_asset).ok_or(Hold::Range)?;
    let paid = to_amount(over, target, token_asset).ok_or(Hold::Range)?;
    if paid.is_zero() {
        return Err(Hold::Dust);
    }
    let to = over.other();
    let pool = &inputs.pool;
    let out = amm::swap_out(pool.holds(over), pool.holds(to), pool.trading_fee, paid)
        .map_err(Hold::Pool)?;
    // Whole drops or millionths, as the other owner book books them: a token's
    // exact price can take every digit a value has, and an amount that
    // leaves the line or the pool more digits than the ledger holds is
    // rounded by the ledger, or refused by the fake.
    let deliver =
        to_amount(to, to_units(out).ok_or(Hold::Range)?, token_asset).ok_or(Hold::Range)?;
    if deliver.is_zero() {
        return Err(Hold::Dust);
    }
    Ok(Rebalance {
        from: over,
        deliver,
        send_max,
    })
}

/// The least `rebalance` can have spent of the asset it spends, in its
/// ledgers' units, if it paid: it may spend a slippage's share more than
/// what it delivers costs at the price read, so it costs at least its
/// `send_max` less that share, and the pool may have moved its way since,
/// by up to another such share. Books of the asset that have fallen by
/// less have not booked it: a move of the node's own between hub and book,
/// on its way, also lowers what they say (spec/areas/rate-feed.md#holdings).
/// At least one unit, so that nothing is booked before anything fell.
#[must_use]
pub fn least_spent(rebalance: &Rebalance, slippage_ppm: u32) -> u64 {
    let most = u128::from(to_units(rebalance.send_max).unwrap_or(0));
    let share = u128::from(MILLION) + u128::from(slippage_ppm);
    let least = most * u128::from(MILLION) / share * u128::from(MILLION) / share;
    u64::try_from(least).unwrap_or(u64::MAX).max(1)
}

/// What `paid` units of `side`'s asset yield of the other's on the pool,
/// in the other's units, rounded down.
fn swap_back(pool: &Pool, side: Side, paid: u64) -> Result<u64, Hold> {
    let amount = to_amount(side, paid, pool.token).ok_or(Hold::Range)?;
    let out = amm::swap_out(
        pool.holds(side),
        pool.holds(side.other()),
        pool.trading_fee,
        amount,
    )
    .map_err(Hold::Pool)?;
    to_units(out).ok_or(Hold::Range)
}

/// `units` of `side`'s ledger as an amount on the XRP Ledger, rounded down
/// to a drop or a millionth; `None` if the ledger cannot hold it.
#[must_use]
pub fn to_amount(side: Side, units: u64, token: IssuedAsset) -> Option<Amount> {
    match side {
        Side::Xrp => Some(Amount::Xrp(units / UNITS_PER_DROP)),
        Side::Token => {
            let value = TokenValue::from_units(units / UNITS_PER_MICRO, MICRO_DECIMALS)?;
            Some(Amount::Token(TokenAmount {
                value,
                currency: token.currency,
                issuer: token.issuer,
            }))
        }
    }
}

/// An amount on the XRP Ledger in its ledger's units, a token rounded down
/// to a millionth; `None` below zero or beyond a `u64`.
#[must_use]
pub fn to_units(amount: Amount) -> Option<u64> {
    match amount {
        Amount::Xrp(drops) => drops.checked_mul(UNITS_PER_DROP),
        Amount::Token(token) => {
            let micros = u64::try_from(token.value.floor_units(MICRO_DECIMALS)?).ok()?;
            micros.checked_mul(UNITS_PER_MICRO)
        }
    }
}

#[cfg(test)]
mod tests {
    use dassie_identity::NODE_ID_BYTES;
    use dassie_xrpl::amount::Currency;
    use dassie_xrpl::keys::AccountId;

    use super::*;

    /// A unit of the ledgers: 10^9 units.
    const ONE: u64 = 1_000_000_000;

    fn token() -> IssuedAsset {
        IssuedAsset {
            currency: Currency::parse("USD").unwrap(),
            issuer: AccountId([7; 20]),
        }
    }

    /// A pool of 1000 XRP and 500 of the token, with a fee of
    /// `trading_fee`: half a dollar an XRP.
    fn pool_with_fee(trading_fee: u32) -> Pool {
        Pool {
            drops: 1_000 * 1_000_000,
            value: TokenValue::parse("500").unwrap(),
            token: token(),
            trading_fee,
        }
    }

    fn pool() -> Pool {
        pool_with_fee(0)
    }

    fn params() -> Params {
        Params {
            curve_ppm: 10_000,
            margin_ppm: 2_000,
            threshold_ppm: 50_000,
            slippage_ppm: 10_000,
            xrp_cap: 20 * ONE,
            token_cap: 10 * ONE,
        }
    }

    /// The pool's price both ways, × 10^15, with no fee.
    const POOL_XRP_TO_TOKEN: u64 = 500_000_000_000_000;
    const POOL_TOKEN_TO_XRP: u64 = 2_000_000_000_000_000;

    /// `rate` raised by `percent`.
    const fn up(rate: u64, percent: u64) -> u64 {
        rate / 100 * (100 + percent)
    }

    fn inputs<'a>(
        targets: &'a [TargetQuote],
        holdings: Holdings,
        params: &'a Params,
    ) -> Inputs<'a> {
        Inputs {
            targets,
            pool: pool(),
            holdings,
            params,
            may_rebalance: true,
        }
    }

    /// Target `n` of a few, in ascending order of their node IDs.
    fn target(n: usize) -> NodeId {
        let mut ids: Vec<NodeId> = (1..=8u8)
            .map(|byte| NodeId::from_bytes(&[byte; NODE_ID_BYTES]))
            .collect();
        ids.sort();
        ids[n]
    }

    /// One target at `rates`.
    fn one(quotes: Quotes) -> [TargetQuote; 1] {
        [quote(0, quotes)]
    }

    fn quote(n: usize, quotes: Quotes) -> TargetQuote {
        TargetQuote {
            target: target(n),
            quotes,
        }
    }

    /// The two hubs' methods.
    const XRP_METHOD: &str = "hub-xrpl-testnet-xrp";
    const TOKEN_METHOD: &str = "hub-xrpl-testnet-dsusd";

    /// A snapshot that quotes the two methods, each with an ask of one and
    /// ladders `depth` deep at a first drop of nothing, and a bid of
    /// `xrp_bid` and `token_bid`, × 10^15.
    fn quoting(xrp_bid: u64, token_bid: u64, depth: u64) -> Quotes {
        use dassie_feed::{LEVELS, Ladder, MethodQuotes, Quote};
        let ladder = Ladder::floor([depth; LEVELS]);
        let method = |id: &str, bid: u64| MethodQuotes {
            tag: method_tag(id),
            bid: Quote::floor(u128::from(bid), u128::from(10u64.pow(15))),
            ask: Quote::ceil(1, 1),
            bid_ladder: ladder,
            ask_ladder: ladder,
        };
        let mut methods = vec![method(XRP_METHOD, xrp_bid), method(TOKEN_METHOD, token_bid)];
        methods.sort_by_key(|method| method.tag);
        let snapshot = Snapshot {
            sequence_ms: 1,
            valid_until_ms: 2,
            grid_bp: [0, 1, 2, 3, 4, 5, 6, 7],
            methods,
        };
        Quotes::new(Arc::new(snapshot), XRP_METHOD, TOKEN_METHOD)
    }

    /// A snapshot whose swaps give `xrp_to_token` and `token_to_xrp`, ×
    /// 10^15, at any size, less the quotes' rounding.
    fn rates(xrp_to_token: u64, token_to_xrp: u64) -> Quotes {
        quoting(xrp_to_token, token_to_xrp, u64::MAX)
    }

    /// A snapshot that prices neither way: it quotes no method.
    fn no_rates() -> Quotes {
        let snapshot = Snapshot {
            sequence_ms: 1,
            valid_until_ms: 2,
            grid_bp: [0, 1, 2, 3, 4, 5, 6, 7],
            methods: Vec::new(),
        };
        Quotes::new(Arc::new(snapshot), XRP_METHOD, TOKEN_METHOD)
    }

    /// Holdings with half of each asset in its hub and half in its book.
    const fn held(xrp: u64, token: u64) -> Holdings {
        Holdings {
            xrp: Holding {
                total: xrp,
                on_ledger: xrp / 2,
            },
            token: Holding {
                total: token,
                on_ledger: token / 2,
            },
        }
    }

    /// Even at the pool's price: 100 XRP and 50 of the token.
    const EVEN: Holdings = held(100 * ONE, 50 * ONE);

    fn trades_of(decision: Decision) -> Vec<Trade> {
        match decision {
            Decision::Trades(trades) => trades,
            other => panic!("trades, not {other:?}"),
        }
    }

    #[test]
    fn no_target_no_trade() {
        let params = params();
        assert_eq!(
            decide(&inputs(&[], EVEN, &params)),
            Decision::Hold(Hold::NoTarget)
        );
        assert_eq!(
            decide(&inputs(&one(no_rates()), EVEN, &params)),
            Decision::Hold(Hold::NoRate)
        );
    }

    // spec/decisions/market-maker-inventory.md: away from even, its own
    // curve values what brings it back more than what it pays, so a trade
    // at the pool's price back toward even clears, and saves the pool's fee
    // a rebalance would pay.
    #[test]
    fn a_trade_at_the_pools_price_back_toward_even_clears() {
        // At 54% in XRP, the curve's weights differ by about 0.16%: more
        // than a margin of 0.1%, less than one of 0.2%.
        let mut params = params();
        params.margin_ppm = 1_000;
        let quotes = one(rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP));
        let skewed = held(108 * ONE, 46 * ONE);
        let trades = trades_of(decide(&inputs(&quotes, skewed, &params)));
        assert!(!trades.is_empty());
        assert!(
            trades.iter().all(|trade| trade.from == Side::Xrp),
            "{trades:?}"
        );
        params.margin_ppm = 2_000;
        assert_eq!(
            decide(&inputs(&quotes, skewed, &params)),
            Decision::Hold(Hold::Margin)
        );
    }

    // spec/decisions/market-maker-inventory.md: at even, a trade either way
    // takes it beyond even, and the pool's price pays nothing over it.
    #[test]
    fn no_trade_at_the_pools_price() {
        let params = params();
        let quotes = one(rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP));
        assert_eq!(
            decide(&inputs(&quotes, EVEN, &params)),
            Decision::Hold(Hold::Margin)
        );
    }

    #[test]
    fn a_target_that_pays_well_gets_a_trade_it_clears() {
        let params = params();
        let quotes = one(rates(POOL_XRP_TO_TOKEN, up(POOL_TOKEN_TO_XRP, 6)));
        let trades = trades_of(decide(&inputs(&quotes, EVEN, &params)));
        let [trade] = trades.as_slice() else {
            panic!("one trade: {trades:?}");
        };
        assert_eq!(trade.target, target(0));
        assert_eq!(trade.from, Side::Token);
        assert!(trade.debit > 0 && trade.debit <= params.token_cap);
        assert!(trade.debit <= EVEN.token.in_hub());
        // It asks for no less than the pool would give, and no more than
        // the snapshot promises.
        assert!(trade.least >= trade.debit * 2, "{trade:?}");
        assert!(trade.least <= trade.receive, "{trade:?}");
    }

    #[test]
    fn a_trade_ends_where_the_ladders_do() {
        let params = params();
        let quotes = one(quoting(
            POOL_XRP_TO_TOKEN,
            up(POOL_TOKEN_TO_XRP, 6),
            3 * ONE,
        ));
        let trades = trades_of(decide(&inputs(&quotes, EVEN, &params)));
        let [trade] = trades.as_slice() else {
            panic!("one trade: {trades:?}");
        };
        assert!(trade.receive <= 3 * ONE, "{trade:?}");
        assert!(trade.debit < params.token_cap, "{trade:?}");
    }

    #[test]
    fn a_trade_pays_at_most_its_cap_and_what_is_in_the_hub() {
        let mut params = params();
        let quotes = one(rates(POOL_XRP_TO_TOKEN, up(POOL_TOKEN_TO_XRP, 6)));
        let mut holdings = EVEN;
        holdings.token.on_ledger = holdings.token.total - 2 * ONE;
        let trades = trades_of(decide(&inputs(&quotes, holdings, &params)));
        assert!(
            trades.iter().all(|trade| trade.debit <= 2 * ONE),
            "{trades:?}"
        );
        params.token_cap = ONE / 2;
        let trades = trades_of(decide(&inputs(&quotes, EVEN, &params)));
        assert!(
            trades.iter().all(|trade| trade.debit <= ONE / 2),
            "{trades:?}"
        );
        // Neither hub holds anything to pay with.
        holdings.token.on_ledger = holdings.token.total;
        holdings.xrp.on_ledger = holdings.xrp.total;
        assert_eq!(
            decide(&inputs(&quotes, holdings, &params)),
            Decision::Hold(Hold::Funds)
        );
    }

    // Each target that clears gets its own trade, and each is valued at the
    // state the ones before it leave: so each asks for at least as much back
    // per unit as the one before, and together they pay no more than the
    // hub holds.
    #[test]
    fn every_target_gets_a_trade_valued_after_the_others() {
        let params = params();
        // Each pays well for the token and badly for XRP, so every trade
        // pays the token.
        let quotes: Vec<TargetQuote> = (0..3)
            .map(|n| {
                quote(
                    n,
                    rates(POOL_XRP_TO_TOKEN / 100 * 95, up(POOL_TOKEN_TO_XRP, 10)),
                )
            })
            .collect();
        let trades = trades_of(decide(&inputs(&quotes, EVEN, &params)));
        assert_eq!(trades.len(), 3, "{trades:?}");
        assert!(trades.iter().all(|trade| trade.from == Side::Token));
        let mut targets: Vec<NodeId> = trades.iter().map(|trade| trade.target).collect();
        targets.sort();
        targets.dedup();
        assert_eq!(targets.len(), 3);
        let paid: u64 = trades.iter().map(|trade| trade.debit).sum();
        assert!(paid <= EVEN.token.in_hub());
        for pair in trades.windows(2) {
            let [before, after] = pair else { continue };
            assert!(
                u128::from(after.least) * u128::from(before.debit)
                    >= u128::from(before.least) * u128::from(after.debit),
                "{trades:?}"
            );
        }
    }

    #[test]
    fn at_most_as_many_trades_as_the_node_swaps_at_once() {
        let mut params = params();
        params.token_cap = ONE / 10;
        let quotes: Vec<TargetQuote> = (0..6)
            .map(|n| quote(n, rates(POOL_XRP_TO_TOKEN, up(POOL_TOKEN_TO_XRP, 10))))
            .collect();
        let trades = trades_of(decide(&inputs(&quotes, EVEN, &params)));
        assert_eq!(trades.len(), MAX_TRADES);
    }

    // spec/decisions/market-maker-inventory.md: beyond the threshold, a
    // rebalance takes it back to even, before any trade.
    #[test]
    fn beyond_the_threshold_it_rebalances_to_even_first() {
        let params = params();
        let quotes = one(rates(up(POOL_XRP_TO_TOKEN, 10), up(POOL_TOKEN_TO_XRP, 10)));
        // 300 XRP, worth 150, and 50 of the token: 75% in XRP.
        let holdings = Holdings {
            xrp: Holding {
                total: 300 * ONE,
                on_ledger: 200 * ONE,
            },
            token: Holding {
                total: 50 * ONE,
                on_ledger: 25 * ONE,
            },
        };
        let Decision::Rebalance(rebalance) = decide(&inputs(&quotes, holdings, &params)) else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Xrp);
        // Half the difference, worth 50: 100 XRP.
        assert_eq!(rebalance.send_max, Amount::Xrp(100_000_000));
        let delivered = to_units(rebalance.deliver).unwrap();
        assert!((44 * ONE..50 * ONE).contains(&delivered), "{delivered}");
    }

    #[test]
    fn within_the_threshold_it_does_not_rebalance() {
        let params = params();
        // 55% in XRP: just within five points.
        assert_eq!(
            decide(&inputs(&[], held(110 * ONE, 45 * ONE), &params)),
            Decision::Hold(Hold::NoTarget)
        );
    }

    // A rebalance that may not go yet, or cannot, leaves the trades back
    // toward even going on, and only those.
    #[test]
    fn a_rebalance_that_cannot_go_leaves_trades_toward_even() {
        let params = params();
        let both = one(rates(up(POOL_XRP_TO_TOKEN, 10), up(POOL_TOKEN_TO_XRP, 10)));
        let skewed = held(300 * ONE, 50 * ONE);
        let mut waiting = inputs(&both, skewed, &params);
        waiting.may_rebalance = false;
        let trades = trades_of(decide(&waiting));
        assert!(!trades.is_empty());
        assert!(
            trades.iter().all(|trade| trade.from == Side::Xrp),
            "{trades:?}"
        );
        // One that pays well only away from even is not taken, and one
        // that pays less than the pool toward even is not either.
        let away = one(rates(
            POOL_XRP_TO_TOKEN / 100 * 95,
            up(POOL_TOKEN_TO_XRP, 10),
        ));
        let mut waiting = inputs(&away, skewed, &params);
        waiting.may_rebalance = false;
        assert_eq!(decide(&waiting), Decision::Hold(Hold::Margin));
        // Nothing in the book to spend: the rebalance cannot go.
        let mut empty_book = skewed;
        empty_book.xrp.on_ledger = 0;
        let trades = trades_of(decide(&inputs(&both, empty_book, &params)));
        assert!(
            trades.iter().all(|trade| trade.from == Side::Xrp),
            "{trades:?}"
        );
    }

    // spec/decisions/market-maker-inventory.md: what takes it beyond even
    // pays what selling it back on the pool would cost; what brings it
    // back toward even does not. With a pool fee of 1%, a target that pays
    // 1.5% over the pool both ways gets the trade back toward even only.
    #[test]
    fn undoing_a_trade_beyond_even_is_charged() {
        let params = params();
        let quotes = one(rates(
            POOL_XRP_TO_TOKEN / 1_000 * 1_015,
            POOL_TOKEN_TO_XRP / 1_000 * 1_015,
        ));
        let mut inputs = inputs(&quotes, held(106 * ONE, 47 * ONE), &params);
        inputs.pool = pool_with_fee(1_000);
        let trades = trades_of(decide(&inputs));
        assert!(!trades.is_empty());
        assert!(
            trades.iter().all(|trade| trade.from == Side::Xrp),
            "{trades:?}"
        );
    }

    #[test]
    fn the_reason_to_hold_is_the_furthest_any_target_got() {
        let params = params();
        let quotes = [
            quote(0, no_rates()),
            quote(1, rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP)),
        ];
        assert_eq!(
            decide(&inputs(&quotes, EVEN, &params)),
            Decision::Hold(Hold::Margin)
        );
    }

    #[test]
    fn a_rebalance_spends_only_what_the_account_holds() {
        let params = params();
        let mut holdings = held(300 * ONE, 50 * ONE);
        holdings.xrp.on_ledger = 30 * ONE;
        let Decision::Rebalance(rebalance) = decide(&inputs(&[], holdings, &params)) else {
            panic!("a rebalance");
        };
        assert_eq!(
            to_units(rebalance.send_max),
            Some(30 * ONE - FEE_HEADROOM_UNITS)
        );
    }

    #[test]
    fn a_rebalance_is_booked_once_the_books_fell_by_what_it_cost() {
        let params = params();
        let Decision::Rebalance(rebalance) =
            decide(&inputs(&[], held(300 * ONE, 50 * ONE), &params))
        else {
            panic!("a rebalance");
        };
        let cost = amm::swap_in(
            pool().holds(Side::Xrp),
            pool().holds(Side::Token),
            0,
            rebalance.deliver,
        )
        .unwrap();
        let least = least_spent(&rebalance, params.slippage_ppm);
        // What it costs at the price read is more, and a slippage's share
        // less than that still counts it booked.
        let cost = to_units(cost).unwrap();
        assert!(least < cost);
        assert!(
            u128::from(cost) * u128::from(MILLION)
                >= u128::from(least) * u128::from(MILLION + params.slippage_ppm - 1)
        );
        // Nothing fell: never booked.
        let dust = Rebalance {
            from: Side::Xrp,
            deliver: Amount::Xrp(0),
            send_max: Amount::Xrp(0),
        };
        assert_eq!(least_spent(&dust, params.slippage_ppm), 1);
    }

    #[test]
    fn units_round_down_to_the_ledger() {
        assert_eq!(to_amount(Side::Xrp, 1_999, token()), Some(Amount::Xrp(1)));
        assert_eq!(to_units(Amount::Xrp(1)), Some(1_000));
        let micro = to_amount(Side::Token, 1_999, token()).unwrap();
        assert_eq!(to_units(micro), Some(1_000));
        assert_eq!(to_units(Amount::Xrp(u64::MAX)), None);
    }
}
