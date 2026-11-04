//! An offer's price on the XRP Ledger's order book
//! (spec/areas/xrpl.md#the-order-book), in exact integer arithmetic, for a
//! client that trades there and for the fake ledger's book.
//!
//! An offer gives `taker_gets` for `taker_pays`. Its quality is what it
//! asks for each unit it gives, `taker_pays / taker_gets`, as rippled keeps
//! it in the key of the offer's book directory: the lower, the better for a
//! taker, and a book is taken best first. Taking part of an offer costs
//! its price, rounded up to what the ledger can express, so its owner never
//! gets less than it asked; what an amount buys is rounded down. Nothing
//! passes through a float.

use alloc::string::{String, ToString};
use core::cmp::Ordering;

use crate::amm::{self, Round, SwapError};
use crate::amount::{Amount, MAX_MANTISSA, MIN_MANTISSA, TokenValue};

/// What a quality's exponent is kept above, in its top byte.
const QUALITY_EXPONENT_BIAS: i32 = 100;
/// The bits of a quality's mantissa.
const QUALITY_MANTISSA_BITS: u64 = (1 << 56) - 1;
/// What the numerator is raised by before a quality's division: ten to the
/// seventeenth, as rippled's `divide`.
const QUALITY_SCALE: u128 = 100_000_000_000_000_000;

/// An amount above zero as rippled holds it for a division: a token's
/// mantissa and exponent, or drops raised to sixteen digits, as rippled
/// raises an amount of XRP; `None` for zero or below.
fn normalized(amount: Amount) -> Option<(u128, i32)> {
    let (mut mantissa, mut exponent) = match amount {
        Amount::Xrp(drops) => (u128::from(drops), 0),
        Amount::Token(token) if token.value.is_negative() => return None,
        Amount::Token(token) => (u128::from(token.value.mantissa()), token.value.exponent()),
    };
    if mantissa == 0 {
        return None;
    }
    // A drop is at least one: at most fifteen steps.
    while mantissa < u128::from(MIN_MANTISSA) {
        mantissa *= 10;
        exponent = exponent.checked_sub(1)?;
    }
    Some((mantissa, exponent))
}

/// The quality of an offer that gives `taker_gets` for `taker_pays`, as
/// rippled writes it into the key of the offer's book directory: their
/// quotient to sixteen digits, from the numerator raised by ten to the
/// seventeenth, truncated and raised by five, then cut to sixteen digits;
/// its exponent above 100 in the top byte, its mantissa below. A lower one
/// is better for a taker. `None` if either is zero or below, or the
/// exponent does not fit its byte.
#[must_use]
pub fn quality(taker_pays: Amount, taker_gets: Amount) -> Option<u64> {
    let (numerator, numerator_exponent) = normalized(taker_pays)?;
    let (denominator, denominator_exponent) = normalized(taker_gets)?;
    // At most 10^17 × 10^17 / 10^15, plus five: within u128.
    let mut mantissa = numerator
        .checked_mul(QUALITY_SCALE)?
        .checked_div(denominator)?
        .checked_add(5)?;
    let mut exponent = numerator_exponent
        .checked_sub(denominator_exponent)?
        .checked_sub(17)?;
    // At most 10^19: four steps down.
    while mantissa > u128::from(MAX_MANTISSA) {
        mantissa /= 10;
        exponent = exponent.checked_add(1)?;
    }
    let biased = u64::try_from(exponent.checked_add(QUALITY_EXPONENT_BIAS)?).ok()?;
    if biased > 0xFF {
        return None;
    }
    Some(biased << 56 | u64::try_from(mantissa).ok()?)
}

/// A quality as rippled writes it in JSON, as it writes a token's value:
/// `book_offers` and `account_offers` give it so.
#[must_use]
pub fn quality_text(quality: u64) -> String {
    let mantissa = quality & QUALITY_MANTISSA_BITS;
    let exponent =
        i32::from(u8::try_from(quality >> 56).unwrap_or(u8::MAX)) - QUALITY_EXPONENT_BIAS;
    TokenValue::new(false, mantissa, exponent).map_or_else(
        || alloc::format!("{mantissa}e{exponent}"),
        |value| value.to_string(),
    )
}

/// Whether an offer that gives `gets` for `pays` crosses a taker's that
/// asks for `wants` and gives `gives`: its price is at most what the taker
/// pays for each unit, `pays × wants ≤ gives × gets`, exactly. `None` if
/// the assets do not face each other, or an amount is below zero.
#[must_use]
pub fn crosses(pays: Amount, gets: Amount, wants: Amount, gives: Amount) -> Option<bool> {
    if pays.asset() != gives.asset() || gets.asset() != wants.asset() {
        return None;
    }
    let product = |a: Amount, b: Amount| amm::mul(amm::dec(a)?, amm::dec(b)?).ok();
    let price = product(pays, wants)?;
    let limit = product(gives, gets)?;
    Some(amm::compare(price, limit) != Ordering::Greater)
}

/// `amount × numerator / denominator`, of `numerator`'s asset, rounded as
/// `round` says to what the ledger can express.
fn scale(
    amount: Amount,
    numerator: Amount,
    denominator: Amount,
    round: Round,
) -> Result<Amount, SwapError> {
    let decimal = |amount: Amount| amm::dec(amount).ok_or(SwapError::Invalid);
    let (amount_dec, numerator_dec, denominator_dec) =
        (decimal(amount)?, decimal(numerator)?, decimal(denominator)?);
    if numerator_dec.mantissa == 0 || denominator_dec.mantissa == 0 {
        return Err(SwapError::Dry);
    }
    amm::quotient(
        amm::mul(amount_dec, numerator_dec)?,
        denominator_dec,
        numerator.asset(),
        round,
    )
}

/// What taking `taken` of what an offer gives costs, at its price of
/// `pays` for `gets`: `taken × pays / gets`, of `pays`'s asset, rounded up.
/// Taking all of `gets` costs all of `pays`.
///
/// # Errors
///
/// [`SwapError`]: `taken` is not of `gets`'s asset or is below zero, the
/// offer gives or asks nothing, or the cost cannot be expressed.
pub fn cost(taken: Amount, pays: Amount, gets: Amount) -> Result<Amount, SwapError> {
    if taken.asset() != gets.asset() {
        return Err(SwapError::Invalid);
    }
    scale(taken, pays, gets, Round::Up)
}

/// What `paid` buys of what an offer gives, at its price of `pays` for
/// `gets`: `paid × gets / pays`, of `gets`'s asset, rounded down, so that
/// its [`cost`] is at most `paid`.
///
/// # Errors
///
/// [`SwapError`]: `paid` is not of `pays`'s asset or is below zero, the
/// offer gives or asks nothing, or the amount cannot be expressed.
pub fn bought(paid: Amount, pays: Amount, gets: Amount) -> Result<Amount, SwapError> {
    if paid.asset() != pays.asset() {
        return Err(SwapError::Invalid);
    }
    scale(paid, gets, pays, Round::Down)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::amount::{Currency, TokenAmount};
    use crate::keys::AccountId;

    fn usd(text: &str) -> Amount {
        Amount::Token(TokenAmount {
            value: TokenValue::parse(text).unwrap(),
            currency: Currency::parse("USD").unwrap(),
            issuer: AccountId([7; 20]),
        })
    }

    fn eur(text: &str) -> Amount {
        Amount::Token(TokenAmount {
            value: TokenValue::parse(text).unwrap(),
            currency: Currency::parse("EUR").unwrap(),
            issuer: AccountId([8; 20]),
        })
    }

    #[test]
    fn qualities_are_rippled_s() {
        // An offer of EUR for USD from rippled's documentation of
        // `book_offers`, whose quality it gives as 1.511056473200875.
        let quality = quality(usd("27.05340557506234"), eur("17.90363633316433")).unwrap();
        assert_eq!(quality_text(quality), "1.511056473200875");
        // XRP in drops for USD: 2 XRP for a dollar is two million drops
        // for each, and a dollar for 2 XRP half a millionth of one.
        assert_eq!(
            quality_text(super::quality(Amount::Xrp(2_000_000), usd("1")).unwrap()),
            "2000000"
        );
        assert_eq!(
            quality_text(super::quality(usd("1"), Amount::Xrp(2_000_000)).unwrap()),
            "0.0000005"
        );
        // Lower is better for a taker, by the bits alone.
        let cheap = super::quality(Amount::Xrp(1_000_000), usd("1")).unwrap();
        let dear = super::quality(Amount::Xrp(1_000_001), usd("1")).unwrap();
        assert!(cheap < dear);
        assert_eq!(super::quality(Amount::Xrp(0), usd("1")), None);
        assert_eq!(super::quality(usd("-1"), Amount::Xrp(1)), None);
    }

    #[test]
    fn an_offer_crosses_a_taker_at_its_price_or_better() {
        // 10 USD for 20 XRP: two XRP a dollar.
        let (pays, gets) = (Amount::Xrp(20_000_000), usd("10"));
        assert_eq!(
            crosses(pays, gets, usd("5"), Amount::Xrp(10_000_000)),
            Some(true)
        );
        assert_eq!(
            crosses(pays, gets, usd("5"), Amount::Xrp(11_000_000)),
            Some(true)
        );
        assert_eq!(
            crosses(pays, gets, usd("5"), Amount::Xrp(9_999_999)),
            Some(false)
        );
        assert_eq!(crosses(pays, gets, Amount::Xrp(1), usd("5")), None);
    }

    #[test]
    fn taking_part_costs_the_price_rounded_up() {
        let (pays, gets) = (Amount::Xrp(20_000_000), usd("3"));
        // A dollar costs 20/3 XRP: 6666666.67 drops, rounded up.
        assert_eq!(cost(usd("1"), pays, gets), Ok(Amount::Xrp(6_666_667)));
        assert_eq!(cost(usd("3"), pays, gets), Ok(pays));
        // And what it pays buys, rounded down, costs no more.
        let got = bought(Amount::Xrp(6_666_667), pays, gets).unwrap();
        assert_eq!(got, usd("1.00000005"));
        assert!(matches!(cost(got, pays, gets), Ok(Amount::Xrp(drops)) if drops <= 6_666_667));
        assert_eq!(cost(Amount::Xrp(1), pays, gets), Err(SwapError::Invalid));
        assert_eq!(bought(usd("1"), pays, gets), Err(SwapError::Invalid));
    }
}
