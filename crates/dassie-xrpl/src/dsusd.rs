//! The `xrpl-testnet-dsusd` settlement scheme (spec/areas/xrpl.md#dsusd):
//! the [token scheme](crate::token) in DSUSD, Dassie's own testnet dollar
//! token, on the account's trust line to DSUSD's issuer, booked on
//! `xrpl-testnet-dsusd+usd`. An owner gets DSUSD by buying it with XRP on
//! the testnet's XRP/DSUSD AMM pool; the node buys none.

use dassie_ledger::LedgerId;

use crate::DROPS_PER_XRP;
use crate::account::LineKey;
use crate::amount::Currency;
use crate::keys::AccountId;
use crate::peg::Peg;
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
    peg: Some(PEG),
};
/// What prices DSUSD: the testnet's DSUSD/XRP pool, which its keeper holds
/// at the market (spec/decisions/dsusd.md), while it holds at least 100
/// XRP. It held about 1,100 when measured, and a pool of a hundred moves a
/// percent for about a DSUSD.
pub const PEG: Peg = Peg {
    min_pool_drops: 100 * DROPS_PER_XRP,
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

    // spec/decisions/mainnet-schemes.md: every XRPL scheme, on either
    // network, disabled test RLUSD included, has an ID, a memo type and a
    // ledger of its own, so a Payment is never taken for a settlement of
    // another, nor test money booked with real; each books on a ledger of
    // its network's realm whose slug is its ID; and no two settle in one
    // asset on one network, nor two tokens on one issuer's line.
    #[test]
    fn no_two_xrpl_schemes_share_a_memo_type_an_issuer_or_a_ledger() {
        use crate::network::Network;
        use crate::rlusd;
        use crate::scheme::{MAINNET_XRP, OWNER_MEMO_TYPE, TESTNET_XRP, Xrp};

        struct Scheme {
            id: &'static str,
            name: dassie_ledger::Name,
            memo_type: &'static [u8],
            ledger: LedgerId,
            network: &'static Network,
            /// The token's line, or `None` for the network's XRP.
            line: Option<LineKey>,
        }
        let xrp = |xrp: Xrp| Scheme {
            id: xrp.scheme_id,
            name: xrp.scheme,
            memo_type: xrp.memo_type,
            ledger: xrp.ledger,
            network: xrp.network,
            line: None,
        };
        let token = |token: Token| Scheme {
            id: token.scheme_id,
            name: token.scheme,
            memo_type: token.memo_type,
            ledger: token.ledger,
            network: token.network,
            line: Some(token.line()),
        };
        let schemes = [
            xrp(TESTNET_XRP),
            xrp(MAINNET_XRP),
            token(rlusd::TOKEN),
            token(rlusd::TESTNET_TOKEN),
            token(TOKEN),
        ];
        for (index, scheme) in schemes.iter().enumerate() {
            let id = scheme.id;
            assert_eq!(scheme.name.as_str(), id);
            assert_eq!(scheme.ledger.realm(), scheme.network.realm, "{id}");
            let slug = scheme.ledger.as_str().split('+').next();
            assert_eq!(slug, Some(id), "the ledger's slug is the scheme's");
            assert_ne!(scheme.memo_type, OWNER_MEMO_TYPE, "{id}");
            if scheme.line.is_some() {
                assert_eq!(scheme.ledger.asset().code, "USD", "{id}");
            }
            for other in schemes.iter().skip(index + 1) {
                let pair = (id, other.id);
                assert_ne!(id, other.id);
                assert_ne!(scheme.memo_type, other.memo_type, "{pair:?}");
                assert_ne!(scheme.ledger, other.ledger, "{pair:?}");
                assert!(
                    scheme.network != other.network || scheme.line != other.line,
                    "{pair:?} settle in one asset on one network"
                );
                if let (Some(line), Some(other_line)) = (scheme.line, other.line) {
                    assert_ne!(line, other_line, "{pair:?}");
                }
            }
        }
    }
}
