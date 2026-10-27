//! The `xrpl-testnet-dsusd` settlement scheme (spec/areas/xrpl.md#dsusd):
//! the [token scheme](crate::token) of DSUSD, Dassie's own testnet dollar
//! token, an owner book that peers with no one, on the account's trust line
//! to DSUSD's issuer, booked on
//! `xrpl-testnet-dsusd+usd`. An owner gets DSUSD by buying it with XRP on
//! the testnet's XRP/DSUSD AMM pool; the node buys none.

use dassie_ledger::LedgerId;

use crate::DROPS_PER_XRP;
use crate::amount::Currency;
use crate::amount::IssuedAsset;
use crate::keys::AccountId;
use crate::peg::Peg;
use crate::token::{IssuedToken, Token, TokenScheme};

pub use crate::token::UNITS_PER_MICRO;

/// The scheme's ID.
pub const SCHEME_ID: &str = "xrpl-testnet-dsusd";
/// DSUSD's issuer on the XRPL testnet, `rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M`.
pub const ISSUER: AccountId = AccountId([
    0x7D, 0x54, 0x40, 0x1D, 0x33, 0xC3, 0xA8, 0xED, 0x1C, 0x9D, 0x61, 0xE2, 0x3D, 0x5D, 0x51, 0xA7,
    0x0E, 0xD2, 0x20, 0x6F,
]);
/// DSUSD's currency code: "DSUSD" in 40 hex digits.
pub const CURRENCY: Currency = Currency([
    0x44, 0x53, 0x55, 0x53, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
/// DSUSD, as the token scheme books it.
pub const TOKEN: Token = Token {
    scheme: dassie_ledger::name!(SCHEME_ID),
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
/// The line the scheme books.
pub const LINE: IssuedAsset = TOKEN.line();

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
    // network, disabled test RLUSD included, has an ID and a ledger of its
    // own, so test money is never booked with real; each books on a ledger
    // of its network's realm whose slug is its ID; and no two book one
    // asset on one network, nor two tokens on one issuer's line. Each ID
    // names the ledger, the network but mainnet, and the asset
    // (spec/decisions/scheme-names.md).
    #[test]
    fn no_two_xrpl_schemes_share_an_issuer_or_a_ledger() {
        use crate::network::Network;
        use crate::rlusd;
        use crate::scheme::{MAINNET_XRP, TESTNET_XRP, Xrp};

        struct Scheme {
            name: dassie_ledger::Name,
            ledger: LedgerId,
            network: &'static Network,
            /// The token's line, or `None` for the network's XRP.
            line: Option<IssuedAsset>,
        }
        let xrp = |xrp: Xrp| Scheme {
            name: xrp.scheme,
            ledger: xrp.ledger,
            network: xrp.network,
            line: None,
        };
        let token = |token: Token| Scheme {
            name: token.scheme,
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
            let id = scheme.name.as_str();
            assert_eq!(scheme.ledger.realm(), scheme.network.realm, "{id}");
            let slug = scheme.ledger.as_str().split('+').next();
            assert_eq!(slug, Some(id), "the ledger's slug is the scheme's");
            let network = match scheme.network.realm {
                dassie_ledger::Realm::Test => "testnet-",
                dassie_ledger::Realm::Live => "",
            };
            let asset = id.rsplit('-').next().unwrap_or_default();
            assert_eq!(id, alloc::format!("xrpl-{network}{asset}"), "the grammar");
            if scheme.line.is_some() {
                assert_eq!(scheme.ledger.asset().code, "USD", "{id}");
            }
            for other in schemes.iter().skip(index + 1) {
                let pair = (id, other.name.as_str());
                assert_ne!(scheme.name, other.name);
                assert_ne!(scheme.ledger, other.ledger, "{pair:?}");
                assert!(
                    scheme.network != other.network || scheme.line != other.line,
                    "{pair:?} book one asset on one network"
                );
                if let (Some(line), Some(other_line)) = (scheme.line, other.line) {
                    assert_ne!(line, other_line, "{pair:?}");
                }
            }
        }
    }
}
