//! An AMM pool's price (spec/areas/xrpl.md#amm-pools): a constant-product
//! pool of two assets with a trading fee, as the XRP Ledger's AMM prices a
//! swap, in exact integer arithmetic.
//!
//! A swap pays `in` of one asset into the pool and takes `out` of the
//! other. The fee is a share of `in` that stays in the pool, and the pool
//! keeps `(pool_in + in × (1 − fee)) × (pool_out − out) ≥ pool_in ×
//! pool_out`. What a swap costs is rounded up, and what it yields rounded
//! down, to what the ledger can express (whole drops, or a token's sixteen
//! significant digits): the rounding always goes to the pool, as the
//! ledger rounds it. Nothing passes through a float.

use core::cmp::Ordering;

use sha2::{Digest, Sha512};

use crate::MAX_DROPS;
use crate::amount::{
    Amount, Asset, Currency, MAX_EXPONENT, MAX_MANTISSA, MIN_EXPONENT, MIN_MANTISSA, TokenAmount,
    TokenValue,
};

/// What a trading fee is counted in: a fee of 1 is a hundred-thousandth.
pub const TRADING_FEE_DENOMINATOR: u32 = 100_000;
/// The largest trading fee the ledger takes: 1 %.
pub const MAX_TRADING_FEE: u32 = 1_000;

/// The most decimal digits a quotient is carried to: a dividend's and a
/// divisor's 39 each, and a mantissa's sixteen.
const MAX_QUOTIENT_DIGITS: u32 = 96;

/// The least product whose square root has sixteen digits: 10^30.
const ROOT_OF_SIXTEEN_DIGITS: u128 = 1_000_000_000_000_000_000_000_000_000_000;

/// Why a swap has no price here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapError {
    /// The amounts are not of the pool's two assets in the order the swap
    /// names them, one is below zero, or the fee is beyond
    /// [`MAX_TRADING_FEE`].
    Invalid,
    /// The pool cannot yield it: it holds none of an asset, or the swap
    /// asks for all it holds of one or more.
    Dry,
    /// The price is beyond what the ledger can express.
    TooLarge,
    /// The amounts are too far apart in size for exact arithmetic in 128
    /// bits.
    Inexact,
}

/// What it costs to take `out` from a pool that holds `pool_in` of the
/// asset paid and `pool_out` of the asset taken, at `trading_fee`
/// ([`TRADING_FEE_DENOMINATOR`]): `pool_in × out / ((pool_out − out) ×
/// (1 − fee))`, of `pool_in`'s asset, rounded up to what the ledger can
/// express.
///
/// # Errors
///
/// [`SwapError`]: the amounts do not fit the pool, it cannot yield `out`,
/// or the price cannot be expressed.
pub fn swap_in(
    pool_in: Amount,
    pool_out: Amount,
    trading_fee: u32,
    out: Amount,
) -> Result<Amount, SwapError> {
    let [pool_in_dec, pool_out_dec, out_dec] = checked(pool_in, pool_out, trading_fee, out)?;
    if out_dec.mantissa == 0 {
        return Ok(zero(pool_in.asset()));
    }
    let left = sub(pool_out_dec, out_dec)?;
    if left.mantissa == 0 {
        return Err(SwapError::Dry);
    }
    let numerator = mul(mul(pool_in_dec, out_dec)?, whole(TRADING_FEE_DENOMINATOR))?;
    let denominator = mul(left, whole(TRADING_FEE_DENOMINATOR - trading_fee))?;
    quotient(numerator, denominator, pool_in.asset(), Round::Up)
}

/// What paying `paid` into a pool that holds `pool_in` of its asset and
/// `pool_out` of the other yields, at `trading_fee`: `pool_out × paid ×
/// (1 − fee) / (pool_in + paid × (1 − fee))`, of `pool_out`'s asset,
/// rounded down to what the ledger can express.
///
/// # Errors
///
/// [`SwapError`]: the amounts do not fit the pool, or the numbers are too
/// far apart to be exact.
pub fn swap_out(
    pool_in: Amount,
    pool_out: Amount,
    trading_fee: u32,
    paid: Amount,
) -> Result<Amount, SwapError> {
    let [pool_in_dec, pool_out_dec, paid_dec] = checked(pool_out, pool_in, trading_fee, paid)
        .map(|[pool_out, pool_in, paid]| [pool_in, pool_out, paid])?;
    let effective = mul(paid_dec, whole(TRADING_FEE_DENOMINATOR - trading_fee))?;
    let numerator = mul(pool_out_dec, effective)?;
    let denominator = add(mul(pool_in_dec, whole(TRADING_FEE_DENOMINATOR))?, effective)?;
    if numerator.mantissa == 0 {
        return Ok(zero(pool_out.asset()));
    }
    quotient(numerator, denominator, pool_out.asset(), Round::Down)
}

/// Whether a pool that held `before` holds at least as much of the two
/// assets' product `after`: what every swap keeps. `None` if an amount is
/// below zero.
#[must_use]
pub fn keeps_product(before: [Amount; 2], after: [Amount; 2]) -> Option<bool> {
    let product = |[a, b]: [Amount; 2]| mul(dec(a)?, dec(b)?).ok();
    Some(compare(product(after)?, product(before)?) != Ordering::Less)
}

/// The currency of a pool's LP token, as the ledger makes it: `0x03`, then
/// the first nineteen bytes of SHA-512Half of the two assets' currencies,
/// the lower first.
#[must_use]
pub fn lp_token_currency(asset: Asset, asset2: Asset) -> Currency {
    let (a, b) = (asset.currency_bits(), asset2.currency_bits());
    let (low, high) = if a <= b { (a, b) } else { (b, a) };
    let mut hasher = Sha512::new();
    hasher.update(low);
    hasher.update(high);
    let digest = hasher.finalize();
    let mut currency = [0; 20];
    currency[0] = 0x03;
    if let (Some(rest), Some(hash)) = (currency.get_mut(1..), digest.get(..19)) {
        rest.copy_from_slice(hash);
    }
    Currency(currency)
}

/// The LP tokens a pool made with `drops` of XRP and `value` of a token
/// issues: the square root of their product, rounded down to sixteen
/// digits; `None` if `value` is below zero.
#[must_use]
pub fn lp_tokens(drops: u64, value: TokenValue) -> Option<TokenValue> {
    if value.is_negative() {
        return None;
    }
    if value.is_zero() || drops == 0 {
        return Some(TokenValue::ZERO);
    }
    // At most 10^17 × 10^16, times ten for an even exponent: within u128.
    let mut product = u128::from(drops).checked_mul(u128::from(value.mantissa()))?;
    let mut exponent = value.exponent();
    if exponent % 2 != 0 {
        product = product.checked_mul(10)?;
        exponent -= 1;
    }
    // A mantissa is at least 10^15, so the product at least that: up by
    // hundreds until its root has sixteen digits, at most eight steps.
    while product < ROOT_OF_SIXTEEN_DIGITS {
        product = product.checked_mul(100)?;
        exponent = exponent.checked_sub(2)?;
    }
    let mut root = product.isqrt();
    let mut exponent = exponent / 2;
    while root > u128::from(MAX_MANTISSA) {
        root /= 10;
        exponent += 1;
    }
    TokenValue::new(false, u64::try_from(root).ok()?, exponent)
}

/// `mantissa × 10^exponent`, not below zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decimal {
    mantissa: u128,
    exponent: i32,
}

fn whole(n: u32) -> Decimal {
    Decimal {
        mantissa: u128::from(n),
        exponent: 0,
    }
}

/// An amount as a decimal: drops are whole; `None` below zero.
fn dec(amount: Amount) -> Option<Decimal> {
    match amount {
        Amount::Xrp(drops) => Some(Decimal {
            mantissa: u128::from(drops),
            exponent: 0,
        }),
        Amount::Token(token) if token.value.is_negative() => None,
        Amount::Token(token) => Some(Decimal {
            mantissa: u128::from(token.value.mantissa()),
            exponent: token.value.exponent(),
        }),
    }
}

/// Nothing of `asset`.
fn zero(asset: Asset) -> Amount {
    match asset {
        Asset::Xrp => Amount::Xrp(0),
        Asset::Token { currency, issuer } => Amount::Token(TokenAmount {
            value: TokenValue::ZERO,
            currency,
            issuer,
        }),
    }
}

/// The pool's two amounts and the one swapped, as decimals, once they fit
/// the pool: two assets, the swapped one of `pool_out`'s, nothing below
/// zero, a fee within the ledger's, and something in the pool.
fn checked(
    pool_in: Amount,
    pool_out: Amount,
    trading_fee: u32,
    amount: Amount,
) -> Result<[Decimal; 3], SwapError> {
    if trading_fee > MAX_TRADING_FEE
        || pool_in.asset() == pool_out.asset()
        || amount.asset() != pool_out.asset()
    {
        return Err(SwapError::Invalid);
    }
    let decimals = [dec(pool_in), dec(pool_out), dec(amount)];
    let [Some(a), Some(b), Some(c)] = decimals else {
        return Err(SwapError::Invalid);
    };
    if a.mantissa == 0 || b.mantissa == 0 {
        return Err(SwapError::Dry);
    }
    Ok([a, b, c])
}

fn mul(a: Decimal, b: Decimal) -> Result<Decimal, SwapError> {
    Ok(Decimal {
        mantissa: a
            .mantissa
            .checked_mul(b.mantissa)
            .ok_or(SwapError::Inexact)?,
        exponent: a
            .exponent
            .checked_add(b.exponent)
            .ok_or(SwapError::Inexact)?,
    })
}

/// Both mantissas at the lower exponent, with it; `None` if the higher one
/// does not fit there. A zero takes the other's exponent.
fn aligned(a: Decimal, b: Decimal) -> Option<(u128, u128, i32)> {
    if a.mantissa == 0 {
        return Some((0, b.mantissa, b.exponent));
    }
    if b.mantissa == 0 {
        return Some((a.mantissa, 0, a.exponent));
    }
    let shift = |d: Decimal, to: i32| {
        let up = u32::try_from(d.exponent.checked_sub(to)?).ok()?;
        d.mantissa.checked_mul(10_u128.checked_pow(up)?)
    };
    let low = a.exponent.min(b.exponent);
    Some((shift(a, low)?, shift(b, low)?, low))
}

fn add(a: Decimal, b: Decimal) -> Result<Decimal, SwapError> {
    let (a, b, exponent) = aligned(a, b).ok_or(SwapError::Inexact)?;
    Ok(Decimal {
        mantissa: a.checked_add(b).ok_or(SwapError::Inexact)?,
        exponent,
    })
}

/// `a − b`, what is left of a pool's `a` once `b` is taken: dry if that
/// is below zero, inexact if the two do not align in 128 bits.
fn sub(a: Decimal, b: Decimal) -> Result<Decimal, SwapError> {
    let (a, b, exponent) = aligned(a, b).ok_or(SwapError::Inexact)?;
    Ok(Decimal {
        mantissa: a.checked_sub(b).ok_or(SwapError::Dry)?,
        exponent,
    })
}

/// How two decimals compare: one that does not fit at the lower exponent
/// is beyond any 128 bits, so above the other.
fn compare(a: Decimal, b: Decimal) -> Ordering {
    match (a.mantissa == 0, b.mantissa == 0) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        (false, false) => {}
    }
    match aligned(a, b) {
        Some((a, b, _)) => a.cmp(&b),
        None => a.exponent.cmp(&b.exponent),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Round {
    Down,
    Up,
}

/// `numerator / denominator`, as an amount of `asset` the ledger can
/// express, rounded as `round` says.
fn quotient(
    numerator: Decimal,
    denominator: Decimal,
    asset: Asset,
    round: Round,
) -> Result<Amount, SwapError> {
    assert!(
        denominator.mantissa > 0,
        "a pool's price divides by its holdings"
    );
    let exponent = numerator
        .exponent
        .checked_sub(denominator.exponent)
        .ok_or(SwapError::Inexact)?;
    match asset {
        Asset::Xrp => {
            drops(numerator.mantissa, denominator.mantissa, exponent, round).map(Amount::Xrp)
        }
        Asset::Token { currency, issuer } => {
            token_value(numerator.mantissa, denominator.mantissa, exponent, round).map(|value| {
                Amount::Token(TokenAmount {
                    value,
                    currency,
                    issuer,
                })
            })
        }
    }
}

/// One more digit of `n / d`: the quotient so far times ten, and what is
/// left.
fn next_digit(quotient: u128, rest: u128, d: u128) -> Result<(u128, u128), SwapError> {
    let rest = rest.checked_mul(10).ok_or(SwapError::Inexact)?;
    let quotient = quotient
        .checked_mul(10)
        .and_then(|q| q.checked_add(rest / d))
        .ok_or(SwapError::TooLarge)?;
    Ok((quotient, rest % d))
}

/// `n / d × 10^exponent` in whole drops, rounded as `round` says.
fn drops(n: u128, d: u128, exponent: i32, round: Round) -> Result<u64, SwapError> {
    let (mut quotient, mut rest) = (n / d, n % d);
    let mut inexact = rest != 0;
    if exponent >= 0 {
        for _ in 0..exponent {
            if quotient > u128::from(MAX_DROPS) {
                return Err(SwapError::TooLarge);
            }
            (quotient, rest) = next_digit(quotient, rest, d)?;
        }
        inexact = rest != 0;
    } else {
        for _ in 0..exponent.unsigned_abs() {
            if quotient == 0 {
                break;
            }
            inexact |= quotient % 10 != 0;
            quotient /= 10;
        }
    }
    if round == Round::Up && inexact {
        quotient = quotient.checked_add(1).ok_or(SwapError::TooLarge)?;
    }
    u64::try_from(quotient)
        .ok()
        .filter(|drops| *drops <= MAX_DROPS)
        .ok_or(SwapError::TooLarge)
}

/// `n / d × 10^exponent` as a token value of sixteen digits, rounded as
/// `round` says.
fn token_value(n: u128, d: u128, exponent: i32, round: Round) -> Result<TokenValue, SwapError> {
    let (mut quotient, mut rest, mut exponent) = (n / d, n % d, exponent);
    // Digits until the mantissa has sixteen: the quotient gains one each
    // step once it is above zero, so at most the divisor's and sixteen.
    for _ in 0..MAX_QUOTIENT_DIGITS {
        if quotient >= u128::from(MIN_MANTISSA) || (quotient == 0 && rest == 0) {
            break;
        }
        (quotient, rest) = next_digit(quotient, rest, d)?;
        exponent = exponent.checked_sub(1).ok_or(SwapError::Inexact)?;
    }
    if quotient == 0 {
        return Ok(TokenValue::ZERO);
    }
    assert!(
        quotient >= u128::from(MIN_MANTISSA),
        "sixteen digits are reached"
    );
    let mut inexact = rest != 0;
    // At most 39 digits: down to sixteen.
    while quotient > u128::from(MAX_MANTISSA) {
        inexact |= quotient % 10 != 0;
        quotient /= 10;
        exponent = exponent.checked_add(1).ok_or(SwapError::TooLarge)?;
    }
    if round == Round::Up && inexact {
        quotient += 1;
        if quotient > u128::from(MAX_MANTISSA) {
            // 9999999999999999 + 1: ten to the sixteenth, exactly.
            quotient /= 10;
            exponent = exponent.checked_add(1).ok_or(SwapError::TooLarge)?;
        }
    }
    if exponent > MAX_EXPONENT {
        return Err(SwapError::TooLarge);
    }
    if exponent < MIN_EXPONENT {
        // Below the least value the ledger holds: nothing, rounded down,
        // and that least value, rounded up.
        return Ok(match round {
            Round::Down => TokenValue::ZERO,
            Round::Up => {
                TokenValue::new(false, MIN_MANTISSA, MIN_EXPONENT).ok_or(SwapError::Inexact)?
            }
        });
    }
    let mantissa = u64::try_from(quotient).map_err(|_| SwapError::TooLarge)?;
    TokenValue::new(false, mantissa, exponent).ok_or(SwapError::Inexact)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::keys::AccountId;

    fn usd(text: &str) -> Amount {
        Amount::Token(TokenAmount {
            value: TokenValue::parse(text).unwrap(),
            currency: Currency::parse("USD").unwrap(),
            issuer: AccountId([7; 20]),
        })
    }

    #[test]
    fn a_swap_costs_what_keeps_the_product_rounded_to_the_pool() {
        // 1000 XRP and 500 USD, no fee: 1 USD out costs
        // 10^9 × 1 / 499 drops, rounded up.
        let (xrp, dollars) = (Amount::Xrp(1_000_000_000), usd("500"));
        assert_eq!(
            swap_in(xrp, dollars, 0, usd("1")),
            Ok(Amount::Xrp(2_004_009))
        );
        // At 1 %, a hundredth more of the input, before rounding.
        assert_eq!(
            swap_in(xrp, dollars, MAX_TRADING_FEE, usd("1")),
            Ok(Amount::Xrp(2_024_251))
        );
        // The other way: 2 XRP out of the pool costs 500 × 2 / 998 USD,
        // up to sixteen digits.
        assert_eq!(
            swap_in(dollars, xrp, 0, Amount::Xrp(2_000_000)),
            Ok(usd("1.002004008016033"))
        );
        // And paying yields the other side, rounded down.
        assert_eq!(
            swap_out(xrp, dollars, 0, Amount::Xrp(2_004_009)),
            Ok(usd("1.000000490017999"))
        );
        assert_eq!(
            swap_out(dollars, xrp, 0, usd("1.002004008016033")),
            Ok(Amount::Xrp(2_000_000))
        );
    }

    #[test]
    fn a_pool_cannot_yield_all_it_holds() {
        let (xrp, dollars) = (Amount::Xrp(1_000), usd("5"));
        assert_eq!(swap_in(xrp, dollars, 0, usd("5")), Err(SwapError::Dry));
        assert_eq!(swap_in(xrp, dollars, 0, usd("6")), Err(SwapError::Dry));
        assert_eq!(
            swap_in(dollars, xrp, 0, Amount::Xrp(1_000)),
            Err(SwapError::Dry)
        );
        assert_eq!(
            swap_in(Amount::Xrp(0), dollars, 0, usd("1")),
            Err(SwapError::Dry)
        );
        // Taking nearly all costs beyond all the XRP there is.
        assert_eq!(
            swap_in(
                Amount::Xrp(MAX_DROPS),
                usd("1"),
                0,
                usd("0.999999999999999")
            ),
            Err(SwapError::TooLarge)
        );
        assert_eq!(swap_in(xrp, dollars, 0, usd("0")), Ok(Amount::Xrp(0)));
        // A tiny amount out of a large pool is no dry pool: the two do not
        // align in 128 bits.
        assert_eq!(
            swap_in(xrp, usd("500"), 0, usd("1e-80")),
            Err(SwapError::Inexact)
        );
    }

    #[test]
    fn prices_below_the_least_value_round_to_zero_or_to_it() {
        let least = Amount::Token(TokenAmount {
            value: TokenValue::new(false, MIN_MANTISSA, MIN_EXPONENT).unwrap(),
            ..usd("1").token().copied().unwrap()
        });
        // A drop into a pool of 10^17 drops and 10^-80 USD yields
        // 10^-97 USD: nothing the ledger holds.
        let (xrp, dollars) = (Amount::Xrp(MAX_DROPS), usd("1e-80"));
        assert_eq!(swap_out(xrp, dollars, 0, Amount::Xrp(1)), Ok(usd("0")));
        // A drop out of a pool of 10^-80 USD and 10^17 drops costs about
        // 10^-97 USD: up to the least value there is.
        assert_eq!(swap_in(dollars, xrp, 0, Amount::Xrp(1)), Ok(least));
    }

    #[test]
    fn swaps_name_the_pools_assets() {
        let (xrp, dollars) = (Amount::Xrp(1_000), usd("5"));
        let invalid = Err(SwapError::Invalid);
        assert_eq!(swap_in(xrp, xrp, 0, Amount::Xrp(1)), invalid);
        assert_eq!(swap_in(xrp, dollars, 0, Amount::Xrp(1)), invalid);
        assert_eq!(swap_out(xrp, dollars, 0, usd("1")), invalid);
        assert_eq!(
            swap_in(xrp, dollars, MAX_TRADING_FEE + 1, usd("1")),
            invalid
        );
        assert_eq!(swap_in(xrp, dollars, 0, usd("-1")), invalid);
    }

    #[test]
    fn products_compare_exactly() {
        let (xrp, dollars) = (Amount::Xrp(1_000_000_000), usd("500"));
        assert_eq!(keeps_product([xrp, dollars], [xrp, dollars]), Some(true));
        let more = [Amount::Xrp(1_002_004_009), usd("499")];
        assert_eq!(keeps_product([xrp, dollars], more), Some(true));
        let less = [Amount::Xrp(1_002_004_008), usd("499")];
        assert_eq!(keeps_product([xrp, dollars], less), Some(false));
        assert_eq!(keeps_product([xrp, usd("-1")], more), None);
    }

    #[test]
    fn lp_tokens_are_the_root_of_the_product() {
        assert_eq!(
            lp_tokens(1_000_000_000, TokenValue::parse("1000").unwrap()),
            TokenValue::parse("1000000")
        );
        assert_eq!(
            lp_tokens(2, TokenValue::parse("1").unwrap()),
            TokenValue::parse("1.414213562373095")
        );
        // The ledger's LP token of XRP and USD starts with 0x03.
        let currency = lp_token_currency(Asset::Xrp, usd("1").asset());
        assert_eq!(currency.0[0], 0x03);
        assert_eq!(currency, lp_token_currency(usd("1").asset(), Asset::Xrp));
    }
}
