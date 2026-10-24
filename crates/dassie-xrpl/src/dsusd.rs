//! The `xrpl-testnet-dsusd` settlement scheme (spec/areas/xrpl.md#dsusd):
//! the [token scheme](crate::token) in DSUSD, Dassie's own testnet dollar
//! token, on the account's trust line to DSUSD's issuer, booked on
//! `xrpl-testnet-dsusd+usd`. An owner gets DSUSD by buying it with XRP on
//! the testnet's XRP/DSUSD AMM pool; the node buys none.

use dassie_ledger::LedgerId;

use crate::account::LineKey;
use crate::amount::Currency;
use crate::keys::AccountId;
use crate::token::{IssuedToken, Token, TokenScheme};

pub use crate::token::UNITS_PER_MICRO;

/// The scheme's ID.
pub const SCHEME_ID: &str = "xrpl-testnet-dsusd";
/// The memo that marks a settlement in DSUSD on the testnet: its own, so a
/// Payment is never read as a settlement of another XRPL scheme. A
/// contract between peers, as the other XRPL schemes' are
/// (spec/areas/xrpl.md#the-payment-it-signs).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/dsusd-testnet/v1";
/// DSUSD's issuer on the XRPL testnet, `rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M`.
pub const ISSUER: AccountId = AccountId([
    0x7D, 0x54, 0x40, 0x1D, 0x33, 0xC3, 0xA8, 0xED, 0x1C, 0x9D, 0x61, 0xE2, 0x3D, 0x5D, 0x51, 0xA7,
    0x0E, 0xD2, 0x20, 0x6F,
]);
/// DSUSD's currency code: "DSUSD" in 40 hex digits.
pub const CURRENCY: Currency = Currency([
    0x44, 0x53, 0x55, 0x53, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
/// DSUSD, as the token scheme settles in it.
pub const TOKEN: Token = Token {
    scheme_id: SCHEME_ID,
    scheme: dassie_ledger::name!(SCHEME_ID),
    memo_type: MEMO_TYPE,
    issuer: ISSUER,
    currency: CURRENCY,
    ledger: LedgerId::XrplTestnetDsusdUsd,
    network: &crate::network::TESTNET,
};
/// The line the scheme settles on.
pub const LINE: LineKey = TOKEN.line();

/// DSUSD, as a type.
#[derive(Clone, Copy, Debug)]
pub enum Dsusd {}

impl IssuedToken for Dsusd {
    const TOKEN: Token = TOKEN;
}

/// The `xrpl-testnet-dsusd` scheme.
pub type DsusdScheme = TokenScheme<Dsusd>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issuer_the_currency_and_the_ledger_are_dsusds() {
        assert_eq!(
            AccountId::parse("rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M"),
            Some(ISSUER)
        );
        assert_eq!(
            Currency::parse("4453555344000000000000000000000000000000"),
            Some(CURRENCY)
        );
        assert_eq!(TOKEN.ledger.as_str(), "xrpl-testnet-dsusd+usd");
        assert_eq!(TOKEN.ledger.asset().code, "USD");
        assert_eq!(TOKEN.scheme.as_str(), SCHEME_ID);
    }

    #[test]
    fn no_xrpl_scheme_shares_a_memo_type_issuer_or_ledger() {
        let tokens = [crate::rlusd::TOKEN, TOKEN];
        assert_ne!(tokens[0].memo_type, tokens[1].memo_type);
        assert_ne!(tokens[0].line(), tokens[1].line());
        assert_ne!(tokens[0].ledger, tokens[1].ledger);
        for token in tokens {
            assert_ne!(token.memo_type, crate::scheme::MEMO_TYPE);
            assert_eq!(token.scheme.as_str(), token.scheme_id);
            let slug = token.ledger.as_str().split('+').next();
            assert_eq!(
                slug,
                Some(token.scheme_id),
                "the ledger's slug is the scheme's"
            );
        }
    }
}
