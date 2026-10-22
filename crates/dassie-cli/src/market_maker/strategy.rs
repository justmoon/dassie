//! What the market maker does next, as a pure function
//! (spec/areas/rate-feed.md#market-maker).
//!
//! Two nodes of the market maker's, pinned to the target, hold one asset
//! each: XRP on one, an issued token on the other. A trade pays from the
//! node of asset `a` through the target to the node of `b`; it pays when
//! the `b` it delivers, swapped back to `a` on the XRP Ledger's AMM pool,
//! comes to more than it cost by the margin. A rebalance moves a node's
//! holding back into its band: one Payment from its XRPL account to the
//! other node's, which delivers the other asset, no more than the other
//! node's band has room for, and spends at most what is above the band's
//! middle, through the pool.
//!
//! Every amount is in the nodes' ledger units: both books count at scale 9
//! (spec/areas/ledger.md#ledgers-and-amounts), so a drop is
//! [`UNITS_PER_DROP`] units and a millionth of the token [`UNITS_PER_MICRO`]
//! units. Converting to the ledger's amounts rounds down, so the market
//! maker never counts on more than it gets. Nothing passes through a float.

use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::amm::{self, SwapError};
use dassie_xrpl::amount::{Amount, Currency, TokenAmount, TokenValue};
use dassie_xrpl::keys::AccountId;

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

/// What the target's feed says it converts at between the two nodes'
/// ledgers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rates {
    /// Its fee, in parts per million of what comes in.
    pub fee_ppm: u32,
    /// From the XRP ledger to the token's, × 10^15, if it converts.
    pub xrp_to_token_e15: Option<u64>,
    /// From the token's ledger to XRP's.
    pub token_to_xrp_e15: Option<u64>,
}

impl Rates {
    /// The rate from `side`'s ledger to the other's.
    #[must_use]
    pub const fn from(&self, side: Side) -> Option<u64> {
        match side {
            Side::Xrp => self.xrp_to_token_e15,
            Side::Token => self.token_to_xrp_e15,
        }
    }
}

/// The token: its currency and issuer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// Its currency.
    pub currency: Currency,
    /// Its issuer.
    pub issuer: AccountId,
}

/// The pool of XRP and the token, as `amm_info` read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool {
    /// Its XRP, in drops.
    pub drops: u64,
    /// Its token.
    pub value: TokenValue,
    /// The token.
    pub token: Token,
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
    /// The owner's balance in its books: what it can send through the
    /// target, and what its band is about.
    pub books: u64,
    /// What its XRPL account holds that it may spend, as its books have
    /// it: what a rebalance can spend now. Less than the books while the
    /// target has not settled what it owes the node yet.
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

/// Everything a decision looks at.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    /// The target's rates, from a current feed.
    pub rates: Rates,
    /// The pool.
    pub pool: Pool,
    /// The nodes' holdings.
    pub holdings: Holdings,
    /// The settings.
    pub params: &'a Params,
}

/// A trade to quote: send `debit` from `from`'s node through the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trade {
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
    /// The feed names no rate between the two ledgers.
    NoRate,
    /// No node may send: each is at the bottom of its band.
    Bands,
    /// No trade clears the margin.
    Margin,
    /// A rebalance would deliver nothing.
    Dust,
    /// A rebalance would take the other node above its band, which would
    /// then be rebalanced back: both hold more than their bands allow.
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
/// that gains most beyond the margin, if any does.
#[must_use]
pub fn decide(inputs: &Inputs<'_>) -> Decision {
    for side in Side::BOTH {
        if inputs.holdings.of(side).books > inputs.params.node(side).band.high {
            return match rebalance(inputs, side) {
                Ok(rebalance) => Decision::Rebalance(rebalance),
                Err(hold) => Decision::Hold(hold),
            };
        }
    }
    trade(inputs)
}

/// The best trade, or why there is none.
fn trade(inputs: &Inputs<'_>) -> Decision {
    let mut best: Option<Trade> = None;
    let mut hold = Hold::NoRate;
    for from in Side::BOTH {
        let Some(rate) = inputs.rates.from(from) else {
            continue;
        };
        let node = inputs.params.node(from);
        let room = inputs.holdings.of(from).books.saturating_sub(node.band.low);
        let debit = node.trade_size.min(room);
        if debit == 0 {
            if hold == Hold::NoRate {
                hold = Hold::Bands;
            }
            continue;
        }
        let Some(receive) = through_target(debit, inputs.rates.fee_ppm, rate) else {
            hold = Hold::Range;
            continue;
        };
        let back = match swap_back(&inputs.pool, from.other(), receive) {
            Ok(back) => back,
            Err(error) => {
                hold = error;
                continue;
            }
        };
        hold = Hold::Margin;
        if !clears(debit, back, inputs.params.margin_ppm) {
            continue;
        }
        let candidate = Trade {
            from,
            debit,
            receive,
            back,
        };
        if best.is_none_or(|best| gains_more(&candidate, &best)) {
            best = Some(candidate);
        }
    }
    best.map_or(Decision::Hold(hold), Decision::Trade)
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
/// is above the band's middle, at most what its account holds now (less
/// the fee's headroom, for XRP), and asks the pool for what a slippage's
/// share less buys, so the price may move by that before it fails. It
/// delivers no more than the other node's band has room for, so that node
/// is not then above its band and rebalanced back, paying the pool's fee
/// each way.
fn rebalance(inputs: &Inputs<'_>, from: Side) -> Result<Rebalance, Hold> {
    let band = inputs.params.node(from).band;
    let holding = inputs.holdings.of(from);
    assert!(holding.books > band.high, "only a node above its band");
    let spendable = match from {
        Side::Xrp => holding.on_ledger.saturating_sub(FEE_HEADROOM_UNITS),
        Side::Token => holding.on_ledger,
    };
    let excess = (holding.books - band.middle()).min(spendable);
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
        .saturating_sub(inputs.holdings.of(to).books);
    let fits = to_amount(to, room, token).ok_or(Hold::Range)?;
    if fits.is_zero() {
        return Err(Hold::Full);
    }
    let pool = &inputs.pool;
    let deliver = amm::swap_out(pool.holds(from), pool.holds(to), pool.trading_fee, paid)
        .map_err(Hold::Pool)?;
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
pub fn to_amount(side: Side, units: u64, token: Token) -> Option<Amount> {
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
    use super::*;

    /// A unit of the ledgers: 10^9 units.
    const ONE: u64 = 1_000_000_000;

    fn token() -> Token {
        Token {
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

    fn inputs<'a>(rates: Rates, holdings: Holdings, params: &'a Params) -> Inputs<'a> {
        Inputs {
            rates,
            pool: pool(),
            holdings,
            params,
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
                books: xrp,
                on_ledger: xrp,
            },
            token: Holding {
                books: token,
                on_ledger: token,
            },
        }
    }

    const MIDDLE: Holdings = held(50 * ONE, 25 * ONE);

    #[test]
    fn no_trade_when_the_target_quotes_the_pools_price() {
        let params = params();
        let decision = decide(&inputs(
            rates(POOL_XRP_TO_TOKEN, POOL_TOKEN_TO_XRP),
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
        let decision = decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), MIDDLE, &params));
        assert_eq!(decision, Decision::Hold(Hold::Margin));
    }

    #[test]
    fn a_trade_when_the_feed_and_the_pool_disagree_beyond_the_margin() {
        let params = params();
        // 2 % more token per XRP than the pool: send XRP.
        let rate = POOL_XRP_TO_TOKEN / 100 * 102;
        let Decision::Trade(trade) =
            decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), MIDDLE, &params))
        else {
            panic!("a trade");
        };
        assert_eq!(trade.from, Side::Xrp);
        assert_eq!(trade.debit, 2 * ONE);
        assert_eq!(trade.receive, 1_020_000_000);
        assert!(clears(trade.debit, trade.back, params.margin_ppm));
        assert!(trade.back > trade.debit);

        // The other way: 2 % more XRP per token than the pool.
        let rate = POOL_TOKEN_TO_XRP / 100 * 102;
        let Decision::Trade(trade) =
            decide(&inputs(rates(POOL_XRP_TO_TOKEN, rate), MIDDLE, &params))
        else {
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
            decide(&inputs(with_fee, MIDDLE, &params)),
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
        let decision = decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), MIDDLE, &params));
        assert!(matches!(decision, Decision::Trade(_)));
        // At the pool's largest fee, 1 %, a 1 % gap no longer clears.
        let mut inputs = inputs(rates(rate, POOL_TOKEN_TO_XRP), MIDDLE, &params);
        inputs.pool.trading_fee = amm::MAX_TRADING_FEE;
        assert_eq!(decide(&inputs), Decision::Hold(Hold::Margin));
    }

    #[test]
    fn the_better_direction_wins() {
        let params = params();
        let decision = decide(&inputs(
            rates(POOL_XRP_TO_TOKEN / 100 * 102, POOL_TOKEN_TO_XRP / 100 * 105),
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
        let Decision::Trade(trade) = decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), near, &params))
        else {
            panic!("a trade");
        };
        assert_eq!(trade.debit, ONE / 2);
        // At the bottom: nothing.
        let bottom = held(10 * ONE, 25 * ONE);
        assert_eq!(
            decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), bottom, &params)),
            Decision::Hold(Hold::Margin)
        );
        let both = held(10 * ONE, 5 * ONE);
        assert_eq!(
            decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), both, &params)),
            Decision::Hold(Hold::Bands)
        );
        // Below the bottom, as after a loss: nothing either.
        let below = held(0, 0);
        assert_eq!(
            decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), below, &params)),
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
            decide(&inputs(none, MIDDLE, &params)),
            Decision::Hold(Hold::NoRate)
        );
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
            decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), full, &params))
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
            decide(&inputs(rates(rate, POOL_TOKEN_TO_XRP), full, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.from, Side::Xrp);
        assert_eq!(rebalance.send_max, Amount::Xrp(65_000_000));
        assert!(matches!(rebalance.deliver, Amount::Token(_)));
    }

    #[test]
    fn a_rebalance_spends_only_what_the_account_holds() {
        let params = params();
        // The books hold 120 XRP, but the target has settled only 30 of
        // them so far: the rebalance spends 30, less the fee's headroom.
        let mut unsettled = held(120 * ONE, 25 * ONE);
        unsettled.xrp.on_ledger = 30 * ONE;
        let Decision::Rebalance(rebalance) = decide(&inputs(rates(1, 1), unsettled, &params))
        else {
            panic!("a rebalance");
        };
        assert_eq!(rebalance.send_max, Amount::Xrp(30_000_000 - MAX_FEE_DROPS));
        // Nothing settled yet: nothing to spend.
        unsettled.xrp.on_ledger = 0;
        assert_eq!(
            decide(&inputs(rates(1, 1), unsettled, &params)),
            Decision::Hold(Hold::Dust)
        );
    }

    #[test]
    fn a_rebalance_never_spends_below_the_band() {
        let params = params();
        for token in [50 * ONE + 1, 51 * ONE, 500 * ONE, u64::MAX / 2] {
            let holdings = held(50 * ONE, token);
            match decide(&inputs(rates(1, 1), holdings, &params)) {
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
        let Decision::Rebalance(rebalance) = decide(&inputs(rates(1, 1), holdings, &params)) else {
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
        let Decision::Rebalance(slipping) = decide(&inputs(rates(1, 1), holdings, &params)) else {
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
            decide(&inputs(rates(1, 1), full, &params)),
            Decision::Hold(Hold::Full)
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
