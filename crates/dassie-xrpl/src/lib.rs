//! The XRP Ledger for Dassie, without IO (spec/areas/xrpl.md).
//!
//! - [`keys`]: the original's wallet, derived from the node's settlement
//!   entropy, so the same phrase finds the same funds; account IDs and
//!   their addresses ([`base58`]).
//! - [`amount`]: amounts of XRP and of tokens, exact, and their
//!   conversion to and from whole units.
//! - [`codec`]: the binary form of the transactions the node sends, a
//!   `Payment` of XRP or of a token with memos and a `TrustSet`, their
//!   signing and their hash.
//! - [`api`]: the rippled WebSocket JSON the node speaks: requests, and
//!   what comes back, parsed and bounded.
//! - [`account`]: the node's XRPL testnet account, an external asset: it
//!   says what to send its ledger, signs, keeps the sequence and what is in
//!   flight, and tells the schemes on it what the ledger says.
//! - [`scheme`]: the `xrpl-testnet` settlement scheme, which settles in the
//!   account's XRP and books what the account tells it.
//! - [`token`]: the token scheme, which settles in a dollar token on the
//!   account's trust line to its issuer, with the account's XRP as gas;
//!   [`rlusd`] is it in RLUSD (`xrpl-testnet-rlusd`), [`dsusd`] in DSUSD
//!   (`xrpl-testnet-dsusd`).
//! - `fake` (feature `fake`): a model of the ledger, for simulations and
//!   tests.
//!
//! Nothing here reads a clock or touches the network
//! (spec/decisions/tigerstyle.md).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod account;
pub mod amm;
pub mod amount;
pub mod api;
pub mod base58;
pub mod codec;
pub mod dsusd;
#[cfg(feature = "fake")]
pub mod fake;
pub mod keys;
pub mod rlusd;
pub mod scheme;
pub mod token;

pub use account::{Config, XrplAccount};
pub use dsusd::DsusdScheme;
pub use keys::{AccountId, Wallet};
pub use rlusd::RlusdScheme;
pub use scheme::XrplScheme;
pub use token::{IssuedToken, Token, TokenScheme};

/// Drops per XRP. The ledger counts in drops, at scale 6.
pub const DROPS_PER_XRP: u64 = 1_000_000;

/// Internal units per drop: the books count XRP at scale 9
/// (spec/areas/ledger.md#ledgers-and-amounts).
pub const UNITS_PER_DROP: u64 = 1_000;

/// The most XRP there is, in drops: 100 billion.
pub const MAX_DROPS: u64 = 100_000_000_000 * DROPS_PER_XRP;

/// Uppercase hex, as the ledger writes hashes and blobs.
#[must_use]
pub fn hex_upper(bytes: &[u8]) -> alloc::string::String {
    let digit = |nibble: u8| {
        char::from(if nibble < 10 {
            b'0' + nibble
        } else {
            b'A' + nibble - 10
        })
    };
    let mut text = alloc::string::String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(digit(byte >> 4));
        text.push(digit(byte & 0x0f));
    }
    text
}

/// Hex in either case, to bytes; `None` if it is not hex.
#[must_use]
pub fn from_hex(text: &str) -> Option<alloc::vec::Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[high, low]| Some(digit(*high)? << 4 | digit(*low)?))
        .collect()
}
