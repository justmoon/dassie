//! What the market maker does next, as a pure function
//! (spec/areas/rate-feed.md#market-maker).
//!
//! Two nodes of the market maker's, each pinned to every target, hold one
//! asset each: XRP on one, an issued token on the other, each in the
//! settlement hub of its asset and in the owner book that funds it. A trade
//! pays from the node of asset `a` through one target to the node of `b`,
//! over their hubs; it pays when the `b` it delivers, swapped back to `a`
//! on the XRP Ledger's AMM pool, comes to more than it cost by the margin.
//! Each target's trade is scored at that target's rates, and the best
//! taken; a target that is not a confirmed uplink of both nodes is passed
//! over. A rebalance moves a node's holding back into its band: one Payment
//! from its owner book's XRPL account to the other node's, which delivers
//! the other asset, no more than the other node's band has room for, and
//! spends at most what is above the band's middle, through the pool.
//!
//! Every amount is in the nodes' ledger units: all their books count at scale 9
//! (spec/areas/ledger.md#ledgers-and-amounts), so a drop is
//! [`UNITS_PER_DROP`] units and a millionth of the token [`UNITS_PER_MICRO`]
//! units. Converting to the ledger's amounts rounds down, so the market
//! maker never counts on more than it gets. Nothing passes through a float.

use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::amm::{self, SwapError};
use dassie_xrpl::amount::{Amount, IssuedAsset, TokenAmount, TokenValue};

/// Ledger units per millionth of the token, as the token scheme books it.
pub const UNITS_PER_MICRO: u64 = dassie_xrpl::token::UNITS_PER_MICRO;

/// The decimals of a millionth.
const MICRO_DECIMALS: u8 = 6;

/// Parts per million.
pub const MILLION: u32 = 1_000_000;

/// A rate's scale: a feed's rates are out per in, × 10^15.
const RATE_SCALE: u128 = 1_000_000_000_000_000;

/// The largest margin a config may ask for: half again what a trade costs.
pub const MAX_MARGIN_PPM: u32 = 500_000;

/// The largest slippage a rebalance may allow: a tenth.
pub const MAX_SLIPPAGE_PPM: u32 = 100_000;

/// The most a rebalance pays the XRP Ledger in fees, in drops: ten times
/// its reference fee. Under a heavier load it waits.
pub const MAX_FEE_DROPS: u64 = 100;

/// What a rebalance from the XRP node leaves its account for the fee.
const FEE_HEADROOM_UNITS: u64 = MAX_FEE_DROPS * UNITS_PER_DROP;

/// Which of the two nodes, by the asset it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The node that holds XRP.
    Xrp,
    /// The node that holds the token.
    Token,
}

impl Side {
    /// Both, in the order decisions look at them.
    pub const BOTH: [Self; 2] = [Self::Xrp, Self::Token];

    /// The other node.
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

/// What a node may hold, in its ledger's units: a trade never takes it
/// below `low`, and above `high` it is rebalanced to the middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    /// The least it keeps.
    pub low: u64,
    /// The most it holds before a rebalance.
    pub high: u64,
}

impl Band {
    /// The middle, where a rebalance takes a node.
    #[must_use]
    pub const fn middle(&self) -> u64 {
        assert!(self.low < self.high, "a checked band");
        self.low + (self.high - self.low) / 2
    }
}

/// How one node trades.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeParams {
    /// The most one trade sends from it, in its ledger's units.
    pub trade_size: u64,
    /// Its band.
    pub band: Band,
}

/// The strategy's settings, from the config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// The least gain a trade must make, in parts per million of what it
    /// sends.
    pub margin_ppm: u32,
    /// How far the pool's price may move against a rebalance before it
    /// fails, in parts per million.
    pub slippage_ppm: u32,
    /// The node that holds XRP.
    pub xrp: NodeParams,
    /// The node that holds the token.
    pub token: NodeParams,
}

impl Params {
    /// The settings of `side`'s node.
    #[must_use]
    pub const fn node(&self, side: Side) -> &NodeParams {
        match side {
            Side::Xrp => &self.xrp,
            Side::Token => &self.token,
        }
    }
}

/// What a target's feed says it converts at between the two nodes'
/// ledgers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    /// Its fee, in parts per million of what comes in.
    pub fee_ppm: u32,
    /// From the XRP hub's method to the token hub's, × 10^15, in their
    /// ledgers' units, if it converts.
    pub xrp_to_token_e15: Option<u64>,
    /// From the token hub's method to the XRP hub's.
    pub token_to_xrp_e15: Option<u64>,
}

impl Rates {
    /// The rate from `side`'s method to the other's.
    #[must_use]
    pub const fn from(&self, side: Side) -> Option<u64> {
        match side {
            Side::Xrp => self.xrp_to_token_e15,
            Side::Token => self.token_to_xrp_e15,
        }
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

/// What one node holds, in its ledger's units, none below zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holding {
    /// What the owner holds of the node's asset, by its books: its funds
    /// in the hub's ledger and in the owner book, short of a move between
    /// them on its way, never above. What it can send through a target, as the node moves
    /// funds into the hub as it needs them, and what its band is about.
    pub total: u64,
    /// What the owner book's XRPL account holds that the owner may spend
    /// now, as its books have it: what a rebalance can spend. Less than the
    /// total by what is in the hub.
    pub on_ledger: u64,
}

/// What each node holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holdings {
    /// The XRP node's.
    pub xrp: Holding,
    /// The token node's.
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

/// What one target offers: its rates, and whether a trade can go through
/// it now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetQuote {
    /// Which target, as the caller counts them; a trade through it names
    /// it.
    pub target: usize,
    /// Its rates, from a current feed.
    pub rates: Rates,
    /// Whether it is a confirmed uplink of both nodes, so the payee's
    /// credentials can name the address beneath it and the payer's first
    /// hop is it (spec/areas/rate-feed.md#market-maker).
    pub peered: bool,
}

/// Everything a decision looks at.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The targets with a current feed, at most
    /// [`MAX_TARGETS`](super::config::MAX_TARGETS).
    pub targets: &'a [TargetQuote],
    /// The target the last trade went through, if any: of trades that gain
    /// alike, the first target after it, in the order of `targets` and
    /// around, is taken, so that targets that pay alike take turns.
    pub last_target: Option<usize>,
    /// The pool.
    pub pool: Pool,
    /// The nodes' holdings.
    pub holdings: Holdings,
    /// The settings.
    pub params: &'a Params,
}

/// A trade to quote: send `debit` from `from`'s node through `target`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trade {
    /// The target it goes through, as its [`TargetQuote`] names it.
    pub target: usize,
    /// The node that pays.
    pub from: Side,
    /// What it sends, in its ledger's units.
    pub debit: u64,
    /// What the feed says the other node receives, in its units.
    pub receive: u64,
    /// What that swaps back to on the pool, in `from`'s units.
    pub back: u64,
}

/// A rebalance: one Payment from `from`'s XRPL account to the other node's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rebalance {
    /// The node whose account pays.
    pub from: Side,
    /// What the Payment delivers, of the other asset.
    pub deliver: Amount,
    /// The most it spends, of `from`'s asset: what is above the band's
    /// middle.
    pub send_max: Amount,
}

/// Why nothing is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// No target with a current feed is a confirmed uplink of both nodes
    /// yet.
    Unpeered,
    /// No feed names a rate between the two methods.
    NoRate,
    /// No node may send: each is at the bottom of its band.
    Bands,
    /// No trade clears the margin.
    Margin,
    /// A rebalance would deliver nothing.
    Dust,
    /// A rebalance would take the other node above its band, which would
    /// then be rebalanced back: the other node has no room in its band, and
    /// no trade from it clears the margin, or it is above its band too.
    Full,
    /// The pool cannot price it.
    Pool(SwapError),
    /// An amount does not fit the ledger's units.
    Range,
}

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Nothing, and why.
    Hold(Hold),
    /// Quote a trade, and pay it if the quote still clears the margin.
    Trade(Trade),
    /// Move a node back into its band on the ledger.
    Rebalance(Rebalance),
}

/// What to do now: a rebalance if a node is above its band, else the trade
/// that gains most beyond the margin through any peered target, if any
/// does. The bands are the nodes', whichever target a trade goes through,
/// so a rebalance does not depend on the targets.
///
/// A rebalance that can move nothing, since the other node has no room in
/// its band or what it would deliver rounds to nothing, does not hold the
/// trades up: the market maker trades instead, but only from the other
/// node into the one above its band, and only while the other is not above
/// its own band. That trade lowers the node that had no room, so the
/// rebalance has room again at the next decision and comes first then; it
/// never takes the other node above its band, so it never leaves both
/// above theirs; and it takes no node below its band, as any trade. With
/// no such trade, it holds for the rebalance's reason.
#[must_use]
pub fn decide(inputs: &Inputs<'_>) -> Decision {
    for side in Side::BOTH {
        if inputs.holdings.of(side).total > inputs.params.node(side).band.high {
            return match rebalance(inputs, side) {
                Ok(rebalance) => Decision::Rebalance(rebalance),
                Err(hold @ (Hold::Full | Hold::Dust)) => {
                    let other = side.other();
                    let within =
                        inputs.holdings.of(other).total <= inputs.params.node(other).band.high;
                    let from: &[Side] = if within { &[other] } else { &[] };
                    match trade(inputs, from) {
                        Decision::Hold(_) => Decision::Hold(hold),
                        decision => decision,
                    }
                }
                Err(hold) => Decision::Hold(hold),
            };
        }
    }
    trade(inputs, &Side::BOTH)
}

/// The best trade from one of the nodes `from` through one of the peered
/// targets, or why there is none: of all the reasons met, the one that got
/// furthest ([`further`]). Of trades that gain alike, the first target's
/// after the last trade's ([`Inputs::last_target`]), and the first node's,
/// is taken.
fn trade(inputs: &Inputs<'_>, from: &[Side]) -> Decision {
    let mut best: Option<Trade> = None;
    let mut hold = Hold::Unpeered;
    // Looked at from the target after the last trade's, around: the first
    // of those alike wins.
    let start = inputs
        .targets
        .iter()
        .position(|quote| Some(quote.target) == inputs.last_target)
        .map_or(0, |last| last + 1);
    let (before, after) = inputs.targets.split_at(start.min(inputs.targets.len()));
    for quote in after.iter().chain(before).filter(|quote| quote.peered) {
        hold = further(hold, Hold::NoRate);
        for &from in from {
            let Some(rate) = quote.rates.from(from) else {
                continue;
            };
            // What a node may send does not depend on the target: its band
            // is its own.
            let node = inputs.params.node(from);
            let room = inputs.holdings.of(from).total.saturating_sub(node.band.low);
            let debit = node.trade_size.min(room);
            if debit == 0 {
                hold = further(hold, Hold::Bands);
                continue;
            }
            let Some(receive) = through_target(debit, quote.rates.fee_ppm, rate) else {
                hold = further(hold, Hold::Range);
                continue;
            };
            let back = match swap_back(&inputs.pool, from.other(), receive) {
                Ok(back) => back,
                Err(error) => {
                    hold = further(hold, error);
                    continue;
                }
            };
            hold = further(hold, Hold::Margin);
            if !clears(debit, back, inputs.params.margin_ppm) {
                continue;
            }
            let candidate = Trade {
                target: quote.target,
                from,
                debit,
                receive,
                back,
            };
            if best.is_none_or(|best| gains_more(&candidate, &best)) {
                best = Some(candidate);
            }
        }
    }
    best.map_or(Decision::Hold(hold), Decision::Trade)
}

/// Of two reasons not to trade, the one a trade got further to: no peered
/// target, then no rate, then no room in the bands, then an amount the
/// ledger or the pool cannot take, and last a trade priced that did not
/// clear the margin. So the reason logged does not depend on the order of
/// the targets.
fn further(a: Hold, b: Hold) -> Hold {
    const fn stage(hold: Hold) -> u8 {
        match hold {
            Hold::Unpeered | Hold::Dust | Hold::Full => 0,
            Hold::NoRate => 1,
            Hold::Bands => 2,
            Hold::Range => 3,
            Hold::Pool(_) => 4,
            Hold::Margin => 5,
        }
    }
    if stage(b) > stage(a) { b } else { a }
}

/// Whether `a` gains more per unit sent than `b`: `a.back / a.debit >
/// b.back / b.debit`, multiplied out.
fn gains_more(a: &Trade, b: &Trade) -> bool {
    u128::from(a.back) * u128::from(b.debit) > u128::from(b.back) * u128::from(a.debit)
}

/// Whether a quoted trade still clears the margin: what the other node
/// would receive, swapped back on the pool, against what `from` sends.
/// Returns what it swaps back to.
///
/// # Errors
///
/// Why it does not: below the margin, or the pool cannot price it.
pub fn check_quote(
    from: Side,
    debit: u64,
    receive: u64,
    pool: &Pool,
    params: &Params,
) -> Result<u64, Hold> {
    let back = swap_back(pool, from.other(), receive)?;
    if debit > 0 && clears(debit, back, params.margin_ppm) {
        Ok(back)
    } else {
        Err(Hold::Margin)
    }
}

/// The rebalance of `from`'s node, which is above its band: it spends what
/// is above the band's middle, at most what its owner book's account holds
/// now (less the fee's headroom, for XRP), and asks the pool for what a
/// slippage's share less buys, so the price may move by that before it
/// fails. It delivers no more than the other node's band has room for, so
/// that node is not then above its band and rebalanced back, paying the
/// pool's fee each way.
fn rebalance(inputs: &Inputs<'_>, from: Side) -> Result<Rebalance, Hold> {
    let band = inputs.params.node(from).band;
    let holding = inputs.holdings.of(from);
    assert!(holding.total > band.high, "only a node above its band");
    let spendable = match from {
        Side::Xrp => holding.on_ledger.saturating_sub(FEE_HEADROOM_UNITS),
        Side::Token => holding.on_ledger,
    };
    let excess = (holding.total - band.middle()).min(spendable);
    let slippage = u128::from(MILLION) + u128::from(inputs.params.slippage_ppm);
    let target = u64::try_from(u128::from(excess) * u128::from(MILLION) / slippage)
        .map_err(|_| Hold::Range)?;
    let token = inputs.pool.token;
    let send_max = to_amount(from, excess, token).ok_or(Hold::Range)?;
    let paid = to_amount(from, target, token).ok_or(Hold::Range)?;
    if paid.is_zero() {
        return Err(Hold::Dust);
    }
    let to = from.other();
    let room = inputs
        .params
        .node(to)
        .band
        .high
        .saturating_sub(inputs.holdings.of(to).total);
    let fits = to_amount(to, room, token).ok_or(Hold::Range)?;
    if fits.is_zero() {
        return Err(Hold::Full);
    }
    let pool = &inputs.pool;
    let out = amm::swap_out(pool.holds(from), pool.holds(to), pool.trading_fee, paid)
        .map_err(Hold::Pool)?;
    // Whole drops or millionths, as the other node books them: a token's
    // exact price can take every digit a value has, and an amount that
    // leaves the line or the pool more digits than the ledger holds is
    // rounded by the ledger, or refused by the fake.
    let deliver = to_amount(to, to_units(out).ok_or(Hold::Range)?, token).ok_or(Hold::Range)?;
    if deliver.is_zero() {
        return Err(Hold::Dust);
    }
    if to_units(deliver).ok_or(Hold::Range)? <= room {
        return Ok(Rebalance {
            from,
            deliver,
            send_max,
        });
    }
    // Only what fits, spending at most a slippage's share more than it
    // costs at the price read: less than the excess it would have spent.
    let cost = amm::swap_in(pool.holds(from), pool.holds(to), pool.trading_fee, fits)
        .map_err(Hold::Pool)?;
    let cost = to_units_up(cost).ok_or(Hold::Range)?;
    let most = u64::try_from(u128::from(cost) * slippage / u128::from(MILLION))
        .map_or(excess, |most| most.min(excess));
    Ok(Rebalance {
        from,
        deliver: fits,
        send_max: to_amount(from, most, token).ok_or(Hold::Range)?,
    })
}

/// The least `rebalance` can have spent of its node's asset, in its
/// ledger's units, if it paid: it may spend a slippage's share more than
/// what it delivers costs at the price read, so it costs at least its
/// `send_max` less that share, and the pool may have moved its way since,
/// by up to another such share. A node's books that have fallen by less
/// have not booked it: a move of the node's own between hub and book, on
/// its way, also lowers what they say (spec/areas/rate-feed.md#bands).
/// At least one unit, so that nothing is booked before anything fell.
#[must_use]
pub fn least_spent(rebalance: &Rebalance, slippage_ppm: u32) -> u64 {
    let most = u128::from(to_units(rebalance.send_max).unwrap_or(0));
    let share = u128::from(MILLION) + u128::from(slippage_ppm);
    let least = most * u128::from(MILLION) / share * u128::from(MILLION) / share;
    u64::try_from(least).unwrap_or(u64::MAX).max(1)
}

/// What `debit` sent through the target delivers at `rate_e15`, as its
/// feed says a packet gets: `floor((debit − fee) × rate / 10^15)`, the fee
/// rounded down. `None` beyond a `u64`.
#[must_use]
pub fn through_target(debit: u64, fee_ppm: u32, rate_e15: u64) -> Option<u64> {
    let fee = u128::from(debit) * u128::from(fee_ppm.min(MILLION)) / u128::from(MILLION);
    let net = u128::from(debit) - fee;
    u64::try_from(net * u128::from(rate_e15) / RATE_SCALE).ok()
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

/// Whether `back` is at least `debit` and the margin:
/// `back × 10^6 ≥ debit × (10^6 + margin)`.
#[must_use]
pub fn clears(debit: u64, back: u64, margin_ppm: u32) -> bool {
    u128::from(back) * u128::from(MILLION)
        >= u128::from(debit) * (u128::from(MILLION) + u128::from(margin_ppm))
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

/// An amount on the XRP Ledger in its ledger's units, a token rounded up
/// to a millionth: what paying it takes. `None` below zero or beyond a
/// `u64`.
fn to_units_up(amount: Amount) -> Option<u64> {
    let down = to_units(amount)?;
    match amount {
        Amount::Xrp(_) => Some(down),
        Amount::Token(token) => {
            let exact = TokenValue::from_units(down / UNITS_PER_MICRO, MICRO_DECIMALS)?;
            if exact == token.value {
                Some(down)
            } else {
                down.checked_add(UNITS_PER_MICRO)
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

    /// A pool of 1000 XRP and 500 of the token, without a fee: half a
    /// dollar an XRP.
    fn pool() -> Pool {
        Pool {
            drops: 1_000 * 1_000_000,
            value: TokenValue::parse("500").unwrap(),
            token: token(),
            trading_fee: 0,
        }
    }

    fn params() -> Params {
        Params {
            margin_ppm: 5_000,
            slippage_ppm: 10_000,
            xrp: NodeParams {
                trade_size: 2 * ONE,
                band: Band {
                    low: 10 * ONE,
                    high: 100 * ONE,
                },
            },
            token: NodeParams {
                trade_size: ONE,
                band: Band {
                    low: 5 * ONE,
                    high: 50 * ONE,
                },
            },
        }
    }

    /// The pool's price both ways, × 10^15, with no fee.
    const POOL_XRP_TO_TOKEN: u64 = 500_000_000_000_000;
    const POOL_TOKEN_TO_XRP: u64 = 2_000_000_000_000_000;

    fn inputs<'a>(
        targets: &'a [TargetQuote],
        holdings: Holdings,
        params: &'a Params,
    ) -> Inputs<'a> {
        Inputs {
            targets,
            last_target: None,
            pool: pool(),
            holdings,
            params,
        }
    }

    /// One target, peered, at `rates`.
    const fn one(rates: Rates) -> [TargetQuote; 1] {
        [quote(0, rates, true)]
    }

    const fn quote(target: usize, rates: Rates, peered: bool) -> TargetQuote {
        TargetQuote {
            target,
            rates,
            peered,
        }
    }

    fn rates(xrp_to_token: u64, token_to_xrp: u64) -> Rates {
        Rates {
            fee_ppm: 0,
            xrp_to_token_e15: Some(xrp_to_token),
            token_to_xrp_e15: Some(token_to_xrp),
        }
    }

    /// Holdings whose accounts hold all the books say.
    const fn held(xrp: u64, token: u64) -> Holdings {
        Holdings {
            xrp: Holding {
                total: xrp,
                on_ledger: xrp,
            },
            token: Holding {
                total: token,
                on_ledger: token,
            },
        }
    }

    const MIDDLE: Holdings = held(50 * ONE, 25 * ONE);

    #[test]
    fn no_trade_when_the_target_quotes_the_pools_price() {
        let params = params();
        let decision = decide(&inputs(
            &one(rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP)),
            MIDDLE,
            &params,
        ));
        // A round trip at the pool's price loses its rounding: nothing
        // clears the margin.
        assert_eq!(decision, Decision::Hold(Hold::Margin));
    }

    #[test]
    fn no_trade_inside_the_margin() {
        let params = params();
        // The target gives 0.4 % more token per XRP than the pool: below
        // the margin of 0.5 %.
        let rate = POOL_XRP_TO_TOKEN / 1_000 * 1_004;
        let decision = decide(&inputs(
            &one(rates(rate, POOL_TOKEN_TO_XRP)),
            MIDDLE,
            &params,
        ));
        assert_eq!(decision, Decision::Hold(Hold::Margin));
    }

    #[test]
    fn a_trade_when_the_feed_and_the_pool_disagree_beyond_the_margin() {
        let params = params();
        // 2 % more token per XRP than the pool: send XRP.
        let rate = POOL_XRP_TO_TOKEN / 100 * 102;
        let Decision::Trade(trade) = decide(&inputs(
            &one(rates(rate, POOL_TOKEN_TO_XRP)),
            MIDDLE,
            &params,
        )) else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Xrp);
        assert_eq!(trade.debit, 2 * ONE);
        assert_eq!(trade.receive, 1_020_000_000);
        assert!(clears(trade.debit, trade.back, params.margin_ppm));
        assert!(trade.back > trade.debit);

        // The other way: 2 % more XRP per token than the pool.
        let rate = POOL_TOKEN_TO_XRP / 100 * 102;
        let Decision::Trade(trade) = decide(&inputs(
            &one(rates(POOL_XRP_TO_TOKEN, rate)),
            MIDDLE,
            &params,
        )) else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Token);
        assert_eq!(trade.debit, ONE);
    }

    #[test]
    fn the_targets_fee_counts_against_the_margin() {
        let params = params();
        let rate = POOL_XRP_TO_TOKEN / 100 * 102;
        let mut with_fee = rates(rate, POOL_TOKEN_TO_XRP);
        // A fee of 2 % eats the gain.
        with_fee.fee_ppm = 20_000;
        assert_eq!(
            decide(&inputs(&one(with_fee), MIDDLE, &params)),
            Decision::Hold(Hold::Margin)
        );
        assert_eq!(
            through_target(1_000, 20_000, 1_000_000_000_000_000),
            Some(980)
        );
        // The fee rounds down, as the connector takes it.
        assert_eq!(through_target(49, 20_000, 1_000_000_000_000_000), Some(49));
    }

    #[test]
    fn the_pools_fee_counts_against_the_margin() {
        let params = params();
        let rate = POOL_XRP_TO_TOKEN / 1_000 * 1_010;
        let decision = decide(&inputs(
            &one(rates(rate, POOL_TOKEN_TO_XRP)),
            MIDDLE,
            &params,
        ));
        assert!(matches!(decision, Decision::Trade(_)));
        // At the pool's largest fee, 1 %, a 1 % gap no longer clears.
        let targets = one(rates(rate, POOL_TOKEN_TO_XRP));
        let mut inputs = inputs(&targets, MIDDLE, &params);
        inputs.pool.trading_fee = amm::MAX_TRADING_FEE;
        assert_eq!(decide(&inputs), Decision::Hold(Hold::Margin));
    }

    #[test]
    fn the_better_direction_wins() {
        let params = params();
        let decision = decide(&inputs(
            &one(rates(
                POOL_XRP_TO_TOKEN / 100 * 102,
                POOL_TOKEN_TO_XRP / 100 * 105,
            )),
            MIDDLE,
            &params,
        ));
        let Decision::Trade(trade) = decision else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Token);
    }

    #[test]
    fn sizes_respect_bands() {
        let params = params();
        let rate = POOL_XRP_TO_TOKEN / 100 * 102;
        // Half a unit above the band's bottom: a trade of half a unit.
        let near = held(10 * ONE + ONE / 2, 25 * ONE);
        let Decision::Trade(trade) =
            decide(&inputs(&one(rates(rate, POOL_TOKEN_TO_XRP)), near, &params))
        else {
            panic!("a trade");
        };
        assert_eq!(trade.debit, ONE / 2);
        // At the bottom: nothing.
        let bottom = held(10 * ONE, 25 * ONE);
        assert_eq!(
            decide(&inputs(
                &one(rates(rate, POOL_TOKEN_TO_XRP)),
                bottom,
                &params
            )),
            Decision::Hold(Hold::Margin)
        );
        let both = held(10 * ONE, 5 * ONE);
        assert_eq!(
            decide(&inputs(&one(rates(rate, POOL_TOKEN_TO_XRP)), both, &params)),
            Decision::Hold(Hold::Bands)
        );
        // Below the bottom, as after a loss: nothing either.
        let below = held(0, 0);
        assert_eq!(
            decide(&inputs(
                &one(rates(rate, POOL_TOKEN_TO_XRP)),
                below,
                &params
            )),
            Decision::Hold(Hold::Bands)
        );
    }

    #[test]
    fn no_rate_no_trade() {
        let params = params();
        let none = Rates {
            fee_ppm: 0,
            xrp_to_token_e15: None,
            token_to_xrp_e15: None,
        };
        assert_eq!(
            decide(&inputs(&one(none), MIDDLE, &params)),
            Decision::Hold(Hold::NoRate)
        );
    }

    #[test]
    fn the_better_target_wins() {
        let params = params();
        // Target 1 gives 2 % more of the token per XRP than the pool,
        // target 2 gives 3 %: the trade goes through 2.
        let good = rates(POOL_XRP_TO_TOKEN / 100 * 102, POOL_TOKEN_TO_XRP);
        let better = rates(POOL_XRP_TO_TOKEN / 100 * 103, POOL_TOKEN_TO_XRP);
        let targets = [quote(1, good, true), quote(2, better, true)];
        let Decision::Trade(trade) = decide(&inputs(&targets, MIDDLE, &params)) else {
            panic!("a trade");
        };
        assert_eq!((trade.target, trade.from), (2, Side::Xrp));
        assert_eq!(trade.receive, 1_030_000_000);
        // In either order.
        let targets = [quote(2, better, true), quote(1, good, true)];
        let Decision::Trade(trade) = decide(&inputs(&targets, MIDDLE, &params)) else {
            panic!("a trade");
        };
        assert_eq!(trade.target, 2);
        // The best of both nodes' trades over both targets: the token node
        // through target 1, at 5 % more XRP per token.
        let other_way = rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP / 100 * 105);
        let targets = [quote(1, other_way, true), quote(2, better, true)];
        let Decision::Trade(trade) = decide(&inputs(&targets, MIDDLE, &params)) else {
            panic!("a trade");
        };
        assert_eq!((trade.target, trade.from), (1, Side::Token));
    }

    #[test]
    fn targets_that_pay_alike_take_turns() {
        let params = params();
        let good = rates(POOL_XRP_TO_TOKEN / 100 * 102, POOL_TOKEN_TO_XRP);
        let targets = [
            quote(4, good, true),
            quote(3, good, false),
            quote(7, good, true),
        ];
        let after = |last| {
            let mut inputs = inputs(&targets, MIDDLE, &params);
            inputs.last_target = last;
            match decide(&inputs) {
                Decision::Trade(trade) => trade.target,
                decision => panic!("a trade, not {decision:?}"),
            }
        };
        // With no trade before, or one through a target gone, the first.
        assert_eq!(after(None), 4);
        assert_eq!(after(Some(9)), 4);
        // Then the next peered one after the last, around.
        assert_eq!(after(Some(4)), 7);
        assert_eq!(after(Some(3)), 7);
        assert_eq!(after(Some(7)), 4);
        // A better one wins whatever came last.
        let better = rates(POOL_XRP_TO_TOKEN / 100 * 103, POOL_TOKEN_TO_XRP);
        let targets = [quote(4, better, true), quote(7, good, true)];
        let mut inputs = inputs(&targets, MIDDLE, &params);
        inputs.last_target = Some(4);
        let Decision::Trade(trade) = decide(&inputs) else {
            panic!("a trade");
        };
        assert_eq!(trade.target, 4);
    }

    #[test]
    fn a_target_not_peered_with_both_nodes_is_passed_over() {
        let params = params();
        let good = rates(POOL_XRP_TO_TOKEN / 100 * 102, POOL_TOKEN_TO_XRP);
        let better = rates(POOL_XRP_TO_TOKEN / 100 * 103, POOL_TOKEN_TO_XRP);
        // The better target is not peered yet: the trade goes through the
        // other.
        let targets = [quote(0, better, false), quote(1, good, true)];
        let Decision::Trade(trade) = decide(&inputs(&targets, MIDDLE, &params)) else {
            panic!("a trade");
        };
        assert_eq!(trade.target, 1);
        assert_eq!(trade.receive, 1_020_000_000);
        // None peered: nothing to trade through.
        let targets = [quote(0, better, false), quote(1, good, false)];
        assert_eq!(
            decide(&inputs(&targets, MIDDLE, &params)),
            Decision::Hold(Hold::Unpeered)
        );
        assert_eq!(
            decide(&inputs(&[], MIDDLE, &params)),
            Decision::Hold(Hold::Unpeered)
        );
        // A rebalance does not need a target: the bands are the nodes'.
        let full = held(120 * ONE, 5 * ONE);
        assert!(matches!(
            decide(&inputs(&targets, full, &params)),
            Decision::Rebalance(Rebalance {
                from: Side::Xrp,
                ..
            })
        ));
    }

    #[test]
    fn a_rebalance_comes_first_whatever_the_targets_offer() {
        let params = params();
        let better = rates(POOL_XRP_TO_TOKEN / 100 * 110, POOL_TOKEN_TO_XRP / 100 * 110);
        let targets = [quote(0, better, true), quote(1, better, true)];
        let full = held(50 * ONE, 60 * ONE);
        assert!(matches!(
            decide(&inputs(&targets, full, &params)),
            Decision::Rebalance(Rebalance {
                from: Side::Token,
                ..
            })
        ));
    }

    #[test]
    fn the_reason_to_hold_is_the_furthest_any_target_got() {
        let params = params();
        let none = Rates {
            fee_ppm: 0,
            xrp_to_token_e15: None,
            token_to_xrp_e15: None,
        };
        let at_pool = rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP);
        // One names no rate, the other's trades miss the margin: whichever
        // comes first, the margin is the reason.
        for targets in [
            [quote(0, none, true), quote(1, at_pool, true)],
            [quote(1, at_pool, true), quote(0, none, true)],
        ] {
            assert_eq!(
                decide(&inputs(&targets, MIDDLE, &params)),
                Decision::Hold(Hold::Margin)
            );
        }
        assert_eq!(further(Hold::Bands, Hold::NoRate), Hold::Bands);
        assert_eq!(further(Hold::Unpeered, Hold::NoRate), Hold::NoRate);
        assert_eq!(further(Hold::Range, Hold::Margin), Hold::Margin);
    }

    #[test]
    fn a_node_above_its_band_rebalances_into_the_other_asset() {
        let params = params();
        let rate = POOL_XRP_TO_TOKEN / 100 * 102;
        // The token node received above its band: it pays XRP to the XRP
        // node, spending what is above its band's middle, 60 − 27.5 = 32.5
        // of the token at most. The XRP node has room for what that buys.
        let full = held(20 * ONE, 60 * ONE);
        let Decision::Rebalance(rebalance) =
            decide(&inputs(&one(rates(rate, POOL_TOKEN_TO_XRP)), full, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Token);
        assert_eq!(
            rebalance.send_max,
            to_amount(Side::Token, 32_500_000_000, token()).unwrap()
        );
        let Amount::Xrp(drops) = rebalance.deliver else {
            panic!("XRP delivered");
        };
        // 32.5 / 1.01 of the token, into 500 of it against 1000 XRP: about
        // 61 XRP, less than the price before the swap would give.
        assert!(drops > 60_000_000 && drops < 65_000_000, "{drops}");
        let cost = amm::swap_in(
            pool().holds(Side::Token),
            pool().holds(Side::Xrp),
            0,
            rebalance.deliver,
        )
        .unwrap();
        // It costs about a slippage's share less than it may spend.
        let (cost, most) = (
            to_units(cost).unwrap(),
            to_units(rebalance.send_max).unwrap(),
        );
        assert!(cost <= most);
        assert!(clears(cost, most, params.slippage_ppm - 1));

        // The XRP node above its band pays the token to the token node.
        let full = held(120 * ONE, 5 * ONE);
        let Decision::Rebalance(rebalance) =
            decide(&inputs(&one(rates(rate, POOL_TOKEN_TO_XRP)), full, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Xrp);
        assert_eq!(rebalance.send_max, Amount::Xrp(65_000_000));
        // In whole millionths, as the token node books it, rounded down
        // from what the pool's exact price would give.
        let Amount::Token(delivered) = rebalance.deliver else {
            panic!("the token delivered");
        };
        let micros = delivered.value.floor_units(MICRO_DECIMALS).unwrap();
        assert_eq!(
            TokenValue::from_units(u64::try_from(micros).unwrap(), MICRO_DECIMALS),
            Some(delivered.value)
        );
        let exact = amm::swap_out(
            pool().holds(Side::Xrp),
            pool().holds(Side::Token),
            0,
            to_amount(Side::Xrp, 65 * ONE * 100 / 101, token()).unwrap(),
        )
        .unwrap();
        let Amount::Token(exact) = exact else {
            panic!("the token out");
        };
        assert_ne!(exact.value, delivered.value, "a price of more digits");
        assert_eq!(to_units(rebalance.deliver), to_units(Amount::Token(exact)));
    }

    #[test]
    fn a_rebalance_is_booked_once_the_books_fell_by_what_it_cost() {
        let params = params();
        let rate = POOL_XRP_TO_TOKEN / 100 * 102;
        let Decision::Rebalance(rebalance) = decide(&inputs(
            &one(rates(rate, POOL_TOKEN_TO_XRP)),
            held(20 * ONE, 60 * ONE),
            &params,
        )) else {
            panic!("a rebalance");
        };
        let cost = amm::swap_in(
            pool().holds(Side::Token),
            pool().holds(Side::Xrp),
            0,
            rebalance.deliver,
        )
        .unwrap();
        let least = least_spent(&rebalance, params.slippage_ppm);
        // What it costs at the price read is more, and a slippage's share
        // less than that still counts it booked.
        let cost = to_units(cost).unwrap();
        assert!(least < cost);
        assert!(clears(least, cost, params.slippage_ppm - 1));
        // Nothing fell: never booked.
        let dust = Rebalance {
            from: Side::Xrp,
            deliver: Amount::Xrp(0),
            send_max: Amount::Xrp(0),
        };
        assert_eq!(least_spent(&dust, params.slippage_ppm), 1);
    }

    #[test]
    fn a_rebalance_spends_only_what_the_account_holds() {
        let params = params();
        // The node holds 120 XRP, but only 30 of them in its owner book's
        // account, the rest in the hub: the rebalance spends 30, less the
        // fee's headroom.
        let mut unsettled = held(120 * ONE, 25 * ONE);
        unsettled.xrp.on_ledger = 30 * ONE;
        let Decision::Rebalance(rebalance) = decide(&inputs(&one(rates(1, 1)), unsettled, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.send_max, Amount::Xrp(30_000_000 - MAX_FEE_DROPS));
        // Everything in the hub: nothing to spend.
        unsettled.xrp.on_ledger = 0;
        assert_eq!(
            decide(&inputs(&one(rates(1, 1)), unsettled, &params)),
            Decision::Hold(Hold::Dust)
        );
    }

    #[test]
    fn a_rebalance_never_spends_below_the_band() {
        let params = params();
        for token in [50 * ONE + 1, 51 * ONE, 500 * ONE, u64::MAX / 2] {
            let holdings = held(50 * ONE, token);
            match decide(&inputs(&one(rates(1, 1)), holdings, &params)) {
                Decision::Rebalance(rebalance) => {
                    let spend = to_units(rebalance.send_max).unwrap();
                    assert!(token - spend >= params.token.band.low);
                }
                Decision::Hold(_) => {}
                Decision::Trade(_) => panic!("a node above its band rebalances first"),
            }
        }
    }

    #[test]
    fn a_rebalance_never_takes_the_other_node_above_its_band() {
        let mut params = params();
        params.slippage_ppm = 0;
        // The token node is 10 above its band, and the XRP node has room
        // for only 5 XRP: it delivers 5, spending what they cost, not the
        // 32.5 of the token above its middle, which would buy about 61 XRP
        // and send the XRP node above its band, to be rebalanced back.
        let holdings = held(95 * ONE, 60 * ONE);
        let Decision::Rebalance(rebalance) = decide(&inputs(&one(rates(1, 1)), holdings, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Token);
        assert_eq!(rebalance.deliver, Amount::Xrp(5_000_000));
        let cost = amm::swap_in(
            pool().holds(Side::Token),
            pool().holds(Side::Xrp),
            0,
            rebalance.deliver,
        )
        .unwrap();
        // SendMax pays what the pool asks, and no more than that rounded
        // up to a millionth.
        let Amount::Token(cost) = cost else {
            panic!("a token cost");
        };
        let Amount::Token(most) = rebalance.send_max else {
            panic!("a token SendMax");
        };
        assert!(cost.value <= most.value);
        assert_eq!(
            to_units(rebalance.send_max),
            to_units_up(Amount::Token(cost))
        );
        // With slippage, a share more, and never more than the excess.
        params.slippage_ppm = 10_000;
        let Decision::Rebalance(slipping) = decide(&inputs(&one(rates(1, 1)), holdings, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(slipping.deliver, Amount::Xrp(5_000_000));
        let (most, slipped) = (
            to_units(rebalance.send_max).unwrap(),
            to_units(slipping.send_max).unwrap(),
        );
        assert_eq!(
            slipped,
            most + most / 100 / UNITS_PER_MICRO * UNITS_PER_MICRO
        );

        // Both above their bands: nothing to rebalance into.
        let full = held(100 * ONE, 60 * ONE);
        assert_eq!(
            decide(&inputs(&one(rates(1, 1)), full, &params)),
            Decision::Hold(Hold::Full)
        );
    }

    /// The bands and sizes the market maker's VM ran with when it stalled:
    /// XRP 20–60, the token 5–40, trades of 2 of each.
    fn vm_params() -> Params {
        Params {
            margin_ppm: 10_000,
            slippage_ppm: 10_000,
            xrp: NodeParams {
                trade_size: 2 * ONE,
                band: Band {
                    low: 20 * ONE,
                    high: 60 * ONE,
                },
            },
            token: NodeParams {
                trade_size: 2 * ONE,
                band: Band {
                    low: 5 * ONE,
                    high: 40 * ONE,
                },
            },
        }
    }

    /// The VM's rates, as its log has them: the target pays 0.8045 XRP for
    /// the token, and 1.179 of the token for an XRP.
    const VM_RATES: Rates = Rates {
        fee_ppm: 0,
        xrp_to_token_e15: Some(1_179_291_322_564_294),
        token_to_xrp_e15: Some(804_523_721_956_600),
    };

    /// A pool at the testnet's price then: 0.76 XRP for the token, with a
    /// fee of half a percent.
    fn vm_inputs<'a>(holdings: Holdings, params: &'a Params) -> Inputs<'a> {
        const TARGETS: [TargetQuote; 1] = one(VM_RATES);
        Inputs {
            targets: &TARGETS,
            last_target: None,
            pool: Pool {
                drops: 7_600 * 1_000_000,
                value: TokenValue::parse("10000").unwrap(),
                token: token(),
                trading_fee: 500,
            },
            holdings,
            params,
        }
    }

    /// The VM's holdings after its first rebalance: the XRP node above its
    /// band's top of 60, the token node at its top of 40.
    const VM_STALLED: Holdings = Holdings {
        xrp: Holding {
            total: 67_371_544_000,
            on_ledger: 66_896_544_000,
        },
        token: Holding {
            total: 40 * ONE,
            on_ledger: 40 * ONE,
        },
    };

    #[test]
    fn a_rebalance_without_room_lets_a_trade_make_room() {
        let params = vm_params();
        let inputs = vm_inputs(VM_STALLED, &params);
        // The XRP node's rebalance has no room in the token node's band:
        // deciding on it alone held, every tick.
        assert_eq!(rebalance(&inputs, Side::Xrp), Err(Hold::Full));
        // The token node sells the token to the target for XRP, which buys
        // more of it back on the pool than it sent: a trade that lowers the
        // node without room.
        let Decision::Trade(trade) = decide(&inputs) else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Token);
        assert_eq!(trade.debit, 2 * ONE);
        assert_eq!(trade.receive, 1_609_047_443);
        assert!(clears(trade.debit, trade.back, params.margin_ppm));

        // Once the target has paid it, the token node has room, and the
        // rebalance comes first again, delivering what fits.
        let traded = Holdings {
            xrp: Holding {
                total: VM_STALLED.xrp.total + trade.receive,
                on_ledger: VM_STALLED.xrp.on_ledger,
            },
            token: Holding {
                total: 38 * ONE,
                on_ledger: 38 * ONE,
            },
        };
        let Decision::Rebalance(rebalance) = decide(&vm_inputs(traded, &params)) else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Xrp);
        assert_eq!(
            rebalance.deliver,
            to_amount(Side::Token, 2 * ONE, token()).unwrap()
        );
    }

    #[test]
    fn a_rebalance_with_room_comes_before_a_trade() {
        let params = vm_params();
        // The token node has room for 10: the XRP node's rebalance moves
        // funds, so it comes first, though the trade would clear the margin.
        let mut room = VM_STALLED;
        room.token = Holding {
            total: 30 * ONE,
            on_ledger: 30 * ONE,
        };
        let Decision::Rebalance(rebalance) = decide(&vm_inputs(room, &params)) else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Xrp);
        assert_eq!(
            rebalance.deliver,
            to_amount(Side::Token, 10 * ONE, token()).unwrap()
        );
        // Below its top, the XRP node makes the same trade as above.
        let mut within = room;
        within.xrp.total = 50 * ONE;
        let Decision::Trade(trade) = decide(&vm_inputs(within, &params)) else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Token);
    }

    #[test]
    fn a_rebalance_without_room_never_trades_into_the_other_node() {
        let params = vm_params();
        // Only the trade the other way clears the margin: from the XRP node
        // into the token node, which has no room. It would take the token
        // node above its band, both above theirs: the rebalance holds.
        let targets = one(Rates {
            fee_ppm: 0,
            xrp_to_token_e15: Some(1_400_000_000_000_000),
            token_to_xrp_e15: Some(700_000_000_000_000),
        });
        let mut inputs = vm_inputs(VM_STALLED, &params);
        inputs.targets = &targets;
        assert_eq!(decide(&inputs), Decision::Hold(Hold::Full));
        // Within its band, the XRP node makes that trade.
        inputs.holdings.xrp.total = 50 * ONE;
        let Decision::Trade(trade) = decide(&inputs) else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Xrp);

        // Both above their bands: no trade either way, as no node could
        // take a rebalance.
        let mut both = VM_STALLED;
        both.token.total = 45 * ONE;
        assert_eq!(
            decide(&vm_inputs(both, &params)),
            Decision::Hold(Hold::Full)
        );
    }

    #[test]
    fn a_rebalance_that_can_spend_nothing_yet_lets_a_trade_go_on() {
        let params = vm_params();
        // Nothing of what the XRP node holds is in its owner book's account
        // yet: its rebalance can spend nothing, and the token node still
        // trades into it, within its band.
        let mut unsettled = VM_STALLED;
        unsettled.xrp.on_ledger = 0;
        unsettled.token = Holding {
            total: 30 * ONE,
            on_ledger: 30 * ONE,
        };
        let inputs = vm_inputs(unsettled, &params);
        assert_eq!(rebalance(&inputs, Side::Xrp), Err(Hold::Dust));
        let Decision::Trade(trade) = decide(&inputs) else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Token);
        // Not below the token node's band.
        unsettled.token.total = 5 * ONE;
        assert_eq!(
            decide(&vm_inputs(unsettled, &params)),
            Decision::Hold(Hold::Dust)
        );
    }

    #[test]
    fn a_quote_is_checked_again() {
        let params = params();
        // 2 units of XRP that deliver 1.02 of the token clear the margin;
        // 1.0 does not.
        assert!(check_quote(Side::Xrp, 2 * ONE, 1_020_000_000, &pool(), &params).is_ok());
        assert_eq!(
            check_quote(Side::Xrp, 2 * ONE, ONE, &pool(), &params),
            Err(Hold::Margin)
        );
        assert_eq!(
            check_quote(Side::Xrp, 0, ONE, &pool(), &params),
            Err(Hold::Margin)
        );
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
