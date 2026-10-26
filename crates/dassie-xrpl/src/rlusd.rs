//! The `xrpl-rlusd` settlement scheme (spec/areas/xrpl.md#rlusd): the
//! [token scheme](crate::token) in RLUSD, Ripple's dollar token, on the
//! trust line of the node's mainnet account to RLUSD's mainnet issuer,
//! booked on `xrpl-rlusd+usd`, a ledger of the live realm: real money.
//!
//! Test RLUSD, `xrpl-testnet-rlusd`, is disabled
//! (spec/decisions/issued-assets.md): no type here settles in it, so no node
//! can activate it. Its constants stay, so that a node that ran it knows
//! its ID, and no other scheme ever takes its memo type or its ledger.

use dassie_ledger::LedgerId;

use crate::DROPS_PER_XRP;
use crate::amount::Currency;
use crate::amount::IssuedAsset;
use crate::keys::AccountId;
use crate::peg::Peg;
use crate::token::{IssuedToken, Token, TokenScheme};

pub use crate::token::UNITS_PER_MICRO;

/// The scheme's ID.
pub const SCHEME_ID: &str = "xrpl-rlusd";
/// The memo that marks a settlement in RLUSD on mainnet: its own, never test
/// RLUSD's, so a Payment is never read as a settlement of another XRPL
/// scheme. A contract between peers, as every XRPL scheme's is
/// (spec/areas/xrpl.md#the-payment-it-signs).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrpl-rlusd/v1";
/// RLUSD's issuer on the XRPL mainnet, `rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De`,
/// as Ripple publishes it.
pub const ISSUER: AccountId = AccountId([
    0xE5, 0xE9, 0x61, 0xC6, 0xA0, 0x25, 0xC9, 0x40, 0x4A, 0xA7, 0xB6, 0x62, 0xDD, 0x1D, 0xF9, 0x75,
    0xBE, 0x75, 0xD1, 0x3E,
]);
/// RLUSD's currency code: "RLUSD" in 40 hex digits, on both networks.
pub const CURRENCY: Currency = Currency([
    0x52, 0x4C, 0x55, 0x53, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
/// RLUSD, as the token scheme settles in it.
pub const TOKEN: Token = Token {
    scheme: dassie_ledger::name!(SCHEME_ID),
    memo_type: MEMO_TYPE,
    issuer: ISSUER,
    currency: CURRENCY,
    ledger: LedgerId::XrplRlusdUsd,
    network: &crate::network::MAINNET,
    peg: Some(PEG),
};
/// What prices RLUSD: mainnet's RLUSD/XRP pool, while it holds at least
/// 100,000 XRP. It held about 1.86 million when measured; drained to a
/// twentieth of that, it has lost the depth that makes its price costly to
/// move.
pub const PEG: Peg = Peg {
    min_pool_drops: 100_000 * DROPS_PER_XRP,
};
/// The line the scheme settles on.
pub const LINE: IssuedAsset = TOKEN.line();

/// RLUSD on mainnet, as a type.
#[derive(Clone, Copy, Debug)]
pub enum Rlusd {}

impl IssuedToken for Rlusd {
    const TOKEN: Token = TOKEN;
}

/// The `xrpl-rlusd` scheme.
pub type RlusdScheme = TokenScheme<Rlusd>;

/// The ID of test RLUSD's scheme, which is disabled.
pub const TESTNET_SCHEME_ID: &str = "xrpl-testnet-rlusd";
/// The memo that would mark a settlement in test RLUSD. Never another
/// scheme's.
pub const TESTNET_MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrpl-testnet-rlusd/v1";
/// RLUSD's issuer on the XRPL testnet, `rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV`.
pub const TESTNET_ISSUER: AccountId = AccountId([
    0xFC, 0xF4, 0xDD, 0x8C, 0x64, 0x63, 0x6B, 0xC5, 0x03, 0xF4, 0xA5, 0x8D, 0xC6, 0xC6, 0x84, 0xD2,
    0xC7, 0xC3, 0xC2, 0x4F,
]);
/// Test RLUSD, as the disabled scheme settled in it: a value only, which no
/// [`IssuedToken`] names.
pub const TESTNET_TOKEN: Token = Token {
    scheme: dassie_ledger::name!(TESTNET_SCHEME_ID),
    memo_type: TESTNET_MEMO_TYPE,
    issuer: TESTNET_ISSUER,
    currency: CURRENCY,
    ledger: LedgerId::XrplTestnetRlusdUsd,
    network: &crate::network::TESTNET,
    // The testnet's RLUSD/XRP pool prices it nowhere near a dollar.
    peg: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issuers_the_currency_and_the_ledgers_are_rlusds() {
        assert_eq!(
            AccountId::parse("rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De"),
            Some(ISSUER)
        );
        assert_eq!(
            AccountId::parse("rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV"),
            Some(TESTNET_ISSUER)
        );
        assert_eq!(
            Currency::parse("524C555344000000000000000000000000000000"),
            Some(CURRENCY)
        );
        assert_eq!(TOKEN.ledger.as_str(), "xrpl-rlusd+usd");
        assert_eq!(TOKEN.ledger.asset().code, "USD");
        assert_eq!(TOKEN.ledger.realm(), dassie_ledger::Realm::Live);
        assert_eq!(TOKEN.scheme.as_str(), SCHEME_ID);
        assert_eq!(TESTNET_TOKEN.ledger.as_str(), "xrpl-testnet-rlusd+usd");
        assert_eq!(TESTNET_TOKEN.ledger.realm(), dassie_ledger::Realm::Test);
        assert_eq!(TESTNET_TOKEN.scheme.as_str(), TESTNET_SCHEME_ID);
    }
}
