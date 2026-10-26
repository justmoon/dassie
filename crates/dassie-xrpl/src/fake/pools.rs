//! AMM pools in the fake ledger (spec/areas/xrpl.md#pools-in-the-fake-ledger):
//! a constant-product pool of XRP and one token, with a trading fee, priced
//! by [`crate::amm`]; `amm_info`; Payments with `SendMax` that cross a
//! pool, XRP for its token or its token for XRP; and the fake's own
//! transaction that makes a pool or sets what it holds, for dev and the
//! end-to-end tests.
//!
//! A pool's assets are its own account's, as on the ledger: its XRP in its
//! account root, its token on a line to the issuer. So the XRP and every
//! token still add up after every ledger, metadata shows what a swap did
//! to each, and a reset wipes the pools with every other account.
//!
//! Arithmetic is exact. What a swap costs is rounded to the pool, as the
//! ledger rounds it, and a token's price further, to a digit the payer's
//! line and the pool's both hold exactly ([`FakeLedger::quote`]), where
//! the ledger would round their balances. A balance the ledger would still
//! have to round fails the transaction (`tecPRECISION_LOSS`), as for any
//! token Payment.

use alloc::string::ToString;

use serde_json::{Value, json};
use sha2::{Digest, Sha512};

use super::tokens::new_line;
use super::{Account, FakeLedger, Outcome, Touched, token_json};
use crate::amm::{self, MAX_TRADING_FEE, SwapError};
use crate::amount::Asset;
use crate::amount::IssuedAsset;
use crate::api::{TrustLine, account_flags};
use crate::codec::{Amount, Currency, Payment, TokenAmount, TokenValue};
use crate::keys::AccountId;

/// A pool account's flags, as the ledger sets them: no master key
/// (`lsfDisableMaster`), lines that ripple, and deposits only by
/// authorisation (`lsfDepositAuth`), so no Payment pays it directly.
pub const POOL_ACCOUNT_FLAGS: u32 = 0x0010_0000 | account_flags::DEFAULT_RIPPLE | 0x0100_0000;

/// A pool of XRP and a token: its account, its trading fee, and the LP
/// tokens it issued as it was made or last set, which no swap changes.
/// What it holds is its account's XRP and its line's balance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Pool {
    pub account: AccountId,
    pub trading_fee: u32,
    pub lp_tokens: TokenValue,
}

/// What a pool holds and charges, as of the last ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolState {
    /// Its account, which holds both assets.
    pub account: AccountId,
    /// Its XRP, in drops.
    pub drops: u64,
    /// Its token.
    pub value: TokenValue,
    /// Its trading fee, in [`amm::TRADING_FEE_DENOMINATOR`]ths.
    pub trading_fee: u32,
}

/// The fake's own transaction, by the token's issuer, that makes the pool
/// of XRP and `token` or sets what it holds and charges. No such
/// transaction exists on the ledger, where liquidity providers deposit and
/// withdraw.
#[derive(Clone, Debug)]
pub(super) struct PoolSet {
    pub account: AccountId,
    pub sequence: u32,
    pub last_ledger_sequence: u32,
    pub token: IssuedAsset,
    pub drops: u64,
    pub value: TokenValue,
    pub trading_fee: u32,
}

/// The account of the pool of XRP and `token`. The ledger derives one
/// from the ledger the pool was made in; the fake from the token alone,
/// the first twenty bytes of SHA-512Half of `AMM`, its issuer and its
/// currency, so a pool made again after a reset has the same account.
#[must_use]
pub fn pool_account(token: IssuedAsset) -> AccountId {
    let mut hasher = Sha512::new();
    hasher.update(b"AMM");
    hasher.update(token.issuer.0);
    hasher.update(token.currency.0);
    let digest = hasher.finalize();
    let mut account = [0; 20];
    account.copy_from_slice(digest.get(..20).unwrap_or(&[0; 20]));
    AccountId(account)
}

/// The most digits a token's price is rounded up by, one at a time: past a
/// mantissa's sixteen and a balance's exponent apart from the price's.
const MAX_ROUNDING_DIGITS: i32 = 48;

/// `cost` rounded up to a whole number of `10^exponent`, if the ledger can
/// express that.
fn round_up_to(cost: TokenValue, exponent: i32) -> Option<TokenValue> {
    let down = exponent.checked_sub(cost.exponent())?;
    if down <= 0 {
        return Some(cost);
    }
    // A mantissa has sixteen digits: past them, one unit of the digit.
    let units = match 10_u64.checked_pow(down.unsigned_abs()) {
        Some(unit) => cost.mantissa().div_ceil(unit),
        None => 1,
    };
    TokenValue::new(false, units, exponent)
}

/// The code a Payment fails with when a pool cannot price it.
const fn swap_failure(error: SwapError) -> &'static str {
    match error {
        // Less than all of it: the ledger's code for a Payment that is not
        // partial and cannot deliver what it names within `SendMax`.
        SwapError::Dry | SwapError::TooLarge => "tecPATH_PARTIAL",
        SwapError::Inexact => "tecPRECISION_LOSS",
        SwapError::Invalid => "tecPATH_DRY",
    }
}

impl FakeLedger {
    /// The issuer of `token` makes its pool with XRP, or sets it, in the
    /// next ledger: it holds `drops` of XRP, from genesis or back to it,
    /// and `value` of the token, minted or taken back, and charges
    /// `trading_fee` ([`amm::TRADING_FEE_DENOMINATOR`]ths).
    ///
    /// # Errors
    ///
    /// The issuer is none of the fake's (`terNO_ACCOUNT`), an amount is not
    /// above zero or beyond the ledger's (`temBAD_AMOUNT`), or the fee is
    /// beyond [`MAX_TRADING_FEE`] (`temBAD_FEE`).
    pub fn set_pool(
        &mut self,
        token: IssuedAsset,
        drops: u64,
        value: TokenValue,
        trading_fee: u32,
    ) -> Result<(), &'static str> {
        let (sequence, last_ledger_sequence) = self.own_slot(token.issuer)?;
        let amount = token.amount(value);
        if drops == 0
            || drops > crate::MAX_DROPS
            || value.is_zero()
            || value.is_negative()
            || !amount.is_valid()
        {
            return Err("temBAD_AMOUNT");
        }
        if trading_fee > MAX_TRADING_FEE {
            return Err("temBAD_FEE");
        }
        self.queue_own(super::Tx::PoolSet(PoolSet {
            account: token.issuer,
            sequence,
            last_ledger_sequence,
            token,
            drops,
            value,
            trading_fee,
        }));
        Ok(())
    }

    /// The pool of XRP and `token`, if there is one.
    #[must_use]
    pub fn pool(&self, token: IssuedAsset) -> Option<PoolState> {
        let pool = self.pools.get(&token)?;
        Some(PoolState {
            account: pool.account,
            drops: self.accounts.get(&pool.account)?.balance,
            value: self.token_balance(pool.account, token)?,
            trading_fee: pool.trading_fee,
        })
    }

    /// What `payer` pays across the pool of XRP and `token` for `out` of
    /// either, in the other: [`amm::swap_in`]'s price. A price in the
    /// token is rounded up further, a digit at a time, until the payer's
    /// line and the pool's hold their balances after it exactly, where the
    /// ledger would round them; it is then above the price by less than a
    /// unit of the last digit the two balances share. An issuer pays from
    /// no line.
    ///
    /// # Errors
    ///
    /// The code a Payment for `out` fails with: no pool (`tecPATH_DRY`),
    /// all it holds or more (`tecPATH_PARTIAL`), or no price exact enough
    /// (`tecPRECISION_LOSS`).
    pub fn quote(
        &self,
        payer: AccountId,
        token: IssuedAsset,
        out: Amount,
    ) -> Result<Amount, &'static str> {
        let pool = self.pool(token).ok_or("tecPATH_DRY")?;
        let (xrp, held) = (
            Amount::Xrp(pool.drops),
            Amount::Token(token.amount(pool.value)),
        );
        let (pool_in, pool_out) = match out {
            Amount::Xrp(_) => (held, xrp),
            Amount::Token(_) => (xrp, held),
        };
        let price = amm::swap_in(pool_in, pool_out, pool.trading_fee, out).map_err(swap_failure)?;
        let Amount::Token(price) = price else {
            return Ok(price);
        };
        let payer_holds = if payer == token.issuer {
            None
        } else {
            self.token_balance(payer, token)
        };
        let exact = |cost: TokenValue| {
            pool.value.checked_add(cost).is_some()
                && payer_holds.is_none_or(|held| held.checked_sub(cost).is_some())
        };
        let first = price.value.exponent();
        for exponent in first..first.saturating_add(MAX_ROUNDING_DIGITS) {
            let cost = round_up_to(price.value, exponent).ok_or("tecPRECISION_LOSS")?;
            if exact(cost) {
                return Ok(Amount::Token(token.amount(cost)));
            }
        }
        Err("tecPRECISION_LOSS")
    }

    /// Whether `account` is a pool's, which takes no Payment.
    pub(super) fn is_pool_account(&self, account: AccountId) -> bool {
        self.pools.values().any(|pool| pool.account == account)
    }

    /// A [`PoolSet`], after its fee.
    pub(super) fn apply_pool_set(&mut self, touched: &mut Touched, set: &PoolSet) -> &'static str {
        let token = set.token;
        let account = pool_account(token);
        let (Some(issuer), Some(mut genesis)) = (
            self.accounts.get(&token.issuer).copied(),
            self.accounts.get(&self.genesis).copied(),
        ) else {
            return "tefINTERNAL";
        };
        // Its XRP from genesis, or back to it.
        let root = self.accounts.get(&account).copied().unwrap_or(Account {
            flags: POOL_ACCOUNT_FLAGS,
            ..Account::new(0, 0)
        });
        if set.drops > root.balance {
            let more = set.drops - root.balance;
            if genesis.balance < more {
                return "tecUNFUNDED";
            }
            genesis.balance -= more;
        } else {
            genesis.balance = genesis.balance.saturating_add(root.balance - set.drops);
        }
        // Its token on its line to the issuer, minted or taken back. Its
        // side holds whatever it is paid, as a pool's does.
        let line = self
            .line(account, token.issuer, token.currency)
            .unwrap_or_else(|| new_line(account, token.issuer, token.currency, &issuer));
        let held = line
            .view(account)
            .map_or(TokenValue::ZERO, |view| view.balance);
        let line = if account == line.low {
            TrustLine {
                low_limit: TokenValue::MAX,
                balance: set.value,
                ..line
            }
        } else {
            TrustLine {
                high_limit: TokenValue::MAX,
                balance: set.value.negate(),
                ..line
            }
        };
        let genesis_id = self.genesis;
        self.put_account(touched, genesis_id, genesis);
        self.put_account(
            touched,
            account,
            Account {
                balance: set.drops,
                ..root
            },
        );
        self.store_line(touched, line);
        let supply = self.supply.entry(token).or_default();
        supply.add(set.value);
        supply.sub(held);
        touched.pool(token, self.pools.get(&token).copied());
        self.pools.insert(
            token,
            Pool {
                account,
                trading_fee: set.trading_fee,
                lp_tokens: amm::lp_tokens(set.drops, set.value).unwrap_or(TokenValue::ZERO),
            },
        );
        "tesSUCCESS"
    }

    /// A Payment, after its fee, that delivers `amount` of a token and
    /// spends at most `max_drops` of XRP: the XRP into the token's pool,
    /// the token out of it to the destination.
    pub(super) fn swap_xrp_for_token(
        &mut self,
        touched: &mut Touched,
        payment: &Payment,
        amount: TokenAmount,
        max_drops: u64,
    ) -> Outcome {
        if !self.accounts.contains_key(&payment.destination) {
            return Outcome::code("tecNO_DST");
        }
        let token = IssuedAsset {
            issuer: amount.issuer,
            currency: amount.currency,
        };
        let Some(pool) = self.pool(token) else {
            return Outcome::code("tecPATH_DRY");
        };
        let paid = match self.quote(payment.account, token, Amount::Token(amount)) {
            Ok(Amount::Xrp(drops)) => drops,
            Ok(Amount::Token(_)) => return Outcome::code("tefINTERNAL"),
            Err(code) => return Outcome::code(code),
        };
        let Some(sender) = self.accounts.get(&payment.account).copied() else {
            return Outcome::code("tefINTERNAL");
        };
        let spare = if payment.account == self.genesis {
            sender.balance
        } else {
            sender.balance.saturating_sub(self.reserve_of(&sender))
        };
        if spare == 0 {
            return Outcome::code("tecPATH_DRY");
        }
        if paid > max_drops || paid > spare {
            return Outcome::code("tecPATH_PARTIAL");
        }
        let planned = match self.plan_token(pool.account, payment.destination, amount) {
            Ok(planned) => planned,
            Err(code) => return Outcome::code(code),
        };
        let Some(root) = self.accounts.get(&pool.account).copied() else {
            return Outcome::code("tefINTERNAL");
        };
        self.put_account(
            touched,
            payment.account,
            Account {
                balance: sender.balance - paid,
                ..sender
            },
        );
        self.put_account(
            touched,
            pool.account,
            Account {
                balance: root.balance.saturating_add(paid),
                ..root
            },
        );
        self.move_token(touched, planned);
        self.check_swap(token, &pool);
        Outcome {
            code: "tesSUCCESS",
            delivered: Some(Amount::Token(amount)),
        }
    }

    /// A Payment, after its fee, that delivers `drops` of XRP and spends at
    /// most `max` of a token: the token into its pool, the XRP out of it
    /// to the destination.
    pub(super) fn swap_token_for_xrp(
        &mut self,
        touched: &mut Touched,
        payment: &Payment,
        drops: u64,
        max: TokenAmount,
    ) -> Outcome {
        if !self.accounts.contains_key(&payment.destination) && drops < self.reserve.0 {
            return Outcome::code("tecNO_DST_INSUF_XRP");
        }
        let token = IssuedAsset {
            issuer: max.issuer,
            currency: max.currency,
        };
        let Some(pool) = self.pool(token) else {
            return Outcome::code("tecPATH_DRY");
        };
        let paid = match self.quote(payment.account, token, Amount::Xrp(drops)) {
            Ok(Amount::Token(paid)) => paid,
            Ok(Amount::Xrp(_)) => return Outcome::code("tefINTERNAL"),
            Err(code) => return Outcome::code(code),
        };
        if paid.value > max.value {
            return Outcome::code("tecPATH_PARTIAL");
        }
        let planned = match self.plan_token(payment.account, pool.account, paid) {
            Ok(planned) => planned,
            Err(code) => return Outcome::code(code),
        };
        self.move_token(touched, planned);
        // The pool's XRP out, to the destination, made if it is new. Both
        // are read after the token moved, which may change an owner count.
        let Some(root) = self.accounts.get(&pool.account).copied() else {
            return Outcome::code("tefINTERNAL");
        };
        self.put_account(
            touched,
            pool.account,
            Account {
                balance: root.balance - drops,
                ..root
            },
        );
        let after = self.accounts.get(&payment.destination).map_or_else(
            || Account::new(drops, self.ledger),
            |destination| Account {
                balance: destination.balance.saturating_add(drops),
                ..*destination
            },
        );
        self.put_account(touched, payment.destination, after);
        self.check_swap(token, &pool);
        Outcome {
            code: "tesSUCCESS",
            delivered: Some(Amount::Xrp(drops)),
        }
    }

    /// The pool of `token`, `before` a swap, holds no smaller product
    /// after it (a bug otherwise: [`amm::swap_in`] rounds to the pool).
    fn check_swap(&self, token: IssuedAsset, before: &PoolState) {
        let after = self.pool(token);
        let kept = after.and_then(|after| {
            amm::keeps_product(
                [
                    Amount::Xrp(before.drops),
                    Amount::Token(token.amount(before.value)),
                ],
                [
                    Amount::Xrp(after.drops),
                    Amount::Token(token.amount(after.value)),
                ],
            )
        });
        assert_eq!(kept, Some(true), "a swap never lowers a pool's product");
    }

    /// `amm_info`: a pool by its two assets, XRP and a token in either
    /// order, or by its account, answered in the order asked.
    pub(super) fn amm_info(&self, request: &Value) -> Result<Value, Value> {
        let malformed = || json!({ "error": "invalidParams" });
        let not_found =
            || json!({ "error": "actNotFound", "ledger_index": self.ledger, "validated": true });
        let (token, xrp_first) = if let Some(account) = request.get("amm_account") {
            let account = account
                .as_str()
                .and_then(AccountId::parse)
                .ok_or_else(malformed)?;
            let token = self
                .pools
                .iter()
                .find(|(_, pool)| pool.account == account)
                .map(|(token, _)| *token)
                .ok_or_else(not_found)?;
            (token, true)
        } else {
            let asset = |key: &str| request.get(key).and_then(asset).ok_or_else(malformed);
            match (asset("asset")?, asset("asset2")?) {
                (Asset::Xrp, Asset::Token { currency, issuer }) => {
                    (IssuedAsset { issuer, currency }, true)
                }
                (Asset::Token { currency, issuer }, Asset::Xrp) => {
                    (IssuedAsset { issuer, currency }, false)
                }
                _ => return Err(not_found()),
            }
        };
        let pool = self.pool(token).ok_or_else(not_found)?;
        let xrp = Value::from(pool.drops.to_string());
        let held = token_json(&token.amount(pool.value));
        let lp_token = TokenAmount {
            value: amm::lp_tokens(pool.drops, pool.value).unwrap_or(TokenValue::ZERO),
            currency: amm::lp_token_currency(
                Asset::Xrp,
                Asset::Token {
                    currency: token.currency,
                    issuer: token.issuer,
                },
            ),
            issuer: pool.account,
        };
        let (amount, amount2, frozen_key) = if xrp_first {
            (xrp, held, "asset2_frozen")
        } else {
            (held, xrp, "asset_frozen")
        };
        let mut amm = json!({
            "account": pool.account.to_address(),
            "amount": amount,
            "amount2": amount2,
            "lp_token": token_json(&lp_token),
            "trading_fee": pool.trading_fee,
            "vote_slots": [],
        });
        if let Some(object) = amm.as_object_mut() {
            object.insert(frozen_key.into(), self.pool_frozen(token, &pool).into());
        }
        Ok(json!({ "amm": amm, "ledger_index": self.ledger, "validated": true }))
    }

    /// Whether the issuer froze its token in the pool: every line, or the
    /// pool's.
    fn pool_frozen(&self, token: IssuedAsset, pool: &PoolState) -> bool {
        let global = self
            .accounts
            .get(&token.issuer)
            .is_some_and(|issuer| issuer.flags & account_flags::GLOBAL_FREEZE != 0);
        let line = self
            .line(pool.account, token.issuer, token.currency)
            .and_then(|line| line.view(token.issuer))
            .is_some_and(|view| view.ours.freeze);
        global || line
    }
}

/// An asset as a request names it: `{"currency": "XRP"}`, or a currency
/// and its issuer.
fn asset(value: &Value) -> Option<Asset> {
    let object = value.as_object()?;
    let currency = object.get("currency")?.as_str()?;
    match object.get("issuer") {
        None if currency == "XRP" => Some(Asset::Xrp),
        None => None,
        Some(issuer) => Some(Asset::Token {
            currency: Currency::parse(currency)?,
            issuer: AccountId::parse(issuer.as_str()?)?,
        }),
    }
}
