//! The `xrpl-testnet-rlusd` settlement scheme (spec/areas/xrpl.md#rlusd):
//! the [token scheme](crate::token) in RLUSD, Ripple's dollar token, on the
//! account's trust line to RLUSD's testnet issuer, booked on
//! `xrpl-testnet-rlusd+usd`.

use dassie_ledger::LedgerId;

use crate::account::LineKey;
use crate::amount::Currency;
use crate::keys::AccountId;
use crate::token::{IssuedToken, Token, TokenScheme};

pub use crate::token::UNITS_PER_MICRO;

/// The scheme's ID.
pub const SCHEME_ID: &str = "xrpl-testnet-rlusd";
/// The memo that marks a settlement in RLUSD on the testnet: its own, so a
/// Payment is never read as a settlement of another XRPL scheme. A
/// contract between peers, as the XRP scheme's is
/// (spec/areas/xrpl.md#the-payment-it-signs).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/rlusd-testnet/v1";
/// RLUSD's issuer on the XRPL testnet, `rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV`.
pub const ISSUER: AccountId = AccountId([
    0xFC, 0xF4, 0xDD, 0x8C, 0x64, 0x63, 0x6B, 0xC5, 0x03, 0xF4, 0xA5, 0x8D, 0xC6, 0xC6, 0x84, 0xD2,
    0xC7, 0xC3, 0xC2, 0x4F,
]);
/// RLUSD's currency code: "RLUSD" in 40 hex digits.
pub const CURRENCY: Currency = Currency([
    0x52, 0x4C, 0x55, 0x53, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
/// RLUSD, as the token scheme settles in it.
pub const TOKEN: Token = Token {
    scheme_id: SCHEME_ID,
    scheme: dassie_ledger::name!(SCHEME_ID),
    memo_type: MEMO_TYPE,
    issuer: ISSUER,
    currency: CURRENCY,
    ledger: LedgerId::XrplTestnetRlusdUsd,
    network: &crate::network::TESTNET,
};
/// The line the scheme settles on.
pub const LINE: LineKey = TOKEN.line();

/// RLUSD, as a type.
#[derive(Clone, Copy, Debug)]
pub enum Rlusd {}

impl IssuedToken for Rlusd {
    const TOKEN: Token = TOKEN;
}

/// The `xrpl-testnet-rlusd` scheme.
pub type RlusdScheme = TokenScheme<Rlusd>;
