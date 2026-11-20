//! An AMM pool of XRP and a token, as one state proof at a validated ledger
//! holds it: the pool's `AMM` object, found by its asset pair, names the
//! pool's account, whose root holds its XRP and whose line to the issuer its
//! token. The node's XRP rate reads mainnet's XRP/RLUSD pool so
//! ([`crate::pool_rate`]).

use alloc::vec::Vec;

use crate::amount::{IssuedAsset, TokenValue};
use crate::anchor::{Doubt, ProvenState};
use crate::api::TrustLine;
use crate::keys::AccountId;
use crate::records;

/// The pool of XRP and a token, as of one validated ledger: both balances
/// at that ledger, never at two, which could pair an old XRP balance with a
/// newer token balance, a ratio the pool never had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolReading {
    /// The pool's account, which holds both.
    pub account: AccountId,
    /// Its XRP, in drops.
    pub drops: u64,
    /// Its token.
    pub value: TokenValue,
    /// The validated ledger both are as of.
    pub ledger_index: u32,
    /// When that ledger closed, by the close time its validators agreed
    /// on, in milliseconds since 1970.
    pub closed_ms: u64,
}

/// What a state proof held of a pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PoolSeen {
    /// There is none.
    Gone,
    /// Its object names this account, other than the one known: the next
    /// proof, with that account's root and line, reads it.
    Named(AccountId),
    /// What it holds.
    Read(PoolReading),
}

/// The keys one state proof proves of the pool of `key`: its `AMM` object,
/// and, once its account is known, that account's root and its line to the
/// issuer. Each is pushed once.
pub(crate) fn push_keys(keys: &mut Vec<[u8; 32]>, key: &IssuedAsset, account: Option<AccountId>) {
    let mut push = |pushed: [u8; 32]| {
        if !keys.contains(&pushed) {
            keys.push(pushed);
        }
    };
    push(records::amm_key(key.issuer, key.currency));
    if let Some(account) = account {
        push(records::account_key(account));
        push(records::line_key(account, key.issuer, key.currency));
    }
}

/// What `proven`, the state of the ledger of `ledger_index` that closed at
/// `close_time` (seconds since 2000), holds of the pool of `key`, whose
/// account was known as `known`: `None` if it did not prove the pool's
/// object.
///
/// # Errors
///
/// [`Doubt::Unreadable`]: a proven object that does not read, an `AMM`
/// object of another pair, or a line between other accounts.
pub(crate) fn seen(
    proven: &ProvenState,
    key: &IssuedAsset,
    known: Option<AccountId>,
    ledger_index: u32,
    close_time: u32,
) -> Result<Option<PoolSeen>, Doubt> {
    let unreadable = |_| Doubt::Unreadable;
    let Some(object) = proven.object(&records::amm_key(key.issuer, key.currency)) else {
        return Ok(None);
    };
    let Some(bytes) = object else {
        return Ok(Some(PoolSeen::Gone));
    };
    let object = records::amm(bytes).map_err(unreadable)?;
    if !object.pairs(key.issuer, key.currency) {
        return Err(Doubt::Unreadable);
    }
    let account = object.account;
    let root = proven.object(&records::account_key(account));
    let line = proven.object(&records::line_key(account, key.issuer, key.currency));
    Ok(Some(match (known == Some(account), root, line) {
        (true, Some(Some(root)), Some(line)) => {
            let root = records::account_root(root).map_err(unreadable)?;
            let value = match line {
                None => TokenValue::ZERO,
                Some(line) => {
                    let (line, _) = records::ripple_state(line).map_err(unreadable)?;
                    tokens(&line, account, key)?
                }
            };
            if root.account != account || value.is_negative() {
                return Err(Doubt::Unreadable);
            }
            PoolSeen::Read(PoolReading {
                account,
                drops: root.balance,
                value,
                ledger_index,
                closed_ms: crate::unl::unix_ms(close_time),
            })
        }
        (true, Some(None), _) => PoolSeen::Gone,
        _ => PoolSeen::Named(account),
    }))
}

/// What the pool's `account` holds of the token of `key` on `line`, the
/// line its key names: one between other accounts, or in another
/// currency, is not what the key holds, as for the account's own lines.
fn tokens(line: &TrustLine, account: AccountId, key: &IssuedAsset) -> Result<TokenValue, Doubt> {
    if !line.is_between(account, key.issuer, key.currency) {
        return Err(Doubt::Unreadable);
    }
    let view = line.view(account).ok_or(Doubt::Unreadable)?;
    Ok(view.balance)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::keys::Wallet;

    // A pool's line is the one between the pool's account and the token's
    // issuer, in the token's currency, at both ends, as each of the
    // account's own lines is: its key binds them.
    #[test]
    fn a_pool_s_line_is_between_the_pool_and_the_issuer() {
        let key = crate::dsusd::LINE;
        let pool = Wallet::from_entropy(&[3; 32]).account();
        let other = Wallet::from_entropy(&[4; 32]).account();
        let value = TokenValue::from_units(12, 0).unwrap();
        let line = |a: AccountId, b: AccountId, currency| {
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            TrustLine {
                low,
                high,
                currency,
                balance: if low == pool { value } else { value.negate() },
                low_limit: TokenValue::ZERO,
                high_limit: TokenValue::ZERO,
                flags: 0,
            }
        };
        assert_eq!(
            tokens(&line(pool, key.issuer, key.currency), pool, &key),
            Ok(value)
        );
        assert_eq!(
            tokens(&line(pool, other, key.currency), pool, &key),
            Err(Doubt::Unreadable)
        );
        assert_eq!(
            tokens(&line(other, key.issuer, key.currency), pool, &key),
            Err(Doubt::Unreadable)
        );
        assert_eq!(
            tokens(
                &line(pool, key.issuer, crate::amount::Currency([9; 20])),
                pool,
                &key
            ),
            Err(Doubt::Unreadable)
        );
    }
}
