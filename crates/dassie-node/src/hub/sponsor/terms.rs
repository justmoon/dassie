//! The terms an owner proposes for a hub it founds: the starting values
//! (`dassie_hub::founding::starting_terms`), each of which the owner may
//! set in JSON:
//!
//! ```json
//! { "fees": { "operation": 10 }, "limits": { "accounts": 2000 }, "timers": { "stall": 7200000 } }
//! ```
//!
//! Fields left out keep their starting value; a field the terms have not
//! is refused, so a typo never founds a hub on a value nobody chose.
//!
//! The hub holds XRP unless `asset` names a token, by its currency code
//! as the XRP Ledger writes it (three characters, or 40 hex digits) and
//! its issuer's address, such as DSUSD's:
//!
//! ```json
//! { "asset": { "currency": "4453555344000000000000000000000000000000", "issuer": "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M" } }
//! ```
//!
//! A token hub's escrow pays its fees in XRP, so two of its starting
//! values differ: no ledger fee, and the least gas
//! ([`LEAST_TOKEN_GAS_DROPS`]) in its place (spec/decisions/token-hubs.md).

use dassie_hub::rules::{Asset, LEAST_TOKEN_GAS_DROPS, Terms};
use dassie_xrpl::AccountId;
use dassie_xrpl::amount::Currency;
use serde::Deserialize;

/// The largest JSON of terms read.
pub const MAX_TERMS_JSON_BYTES: usize = 8 * 1024;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Overrides {
    asset: Option<Token>,
    fees: Option<Fees>,
    limits: Option<Limits>,
    timers: Option<Timers>,
}

/// The token a hub holds, as the XRP Ledger writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Token {
    currency: String,
    issuer: String,
}

impl Token {
    fn asset(&self) -> Result<Asset, String> {
        let currency = Currency::parse(&self.currency).ok_or_else(|| {
            format!(
                "the currency {:?} is no token's code: three characters or 40 hex digits",
                self.currency
            )
        })?;
        let issuer = AccountId::parse(&self.issuer)
            .ok_or_else(|| format!("the issuer {:?} is no XRP Ledger address", self.issuer))?;
        Ok(Asset::Token { currency, issuer })
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Fees {
    operation: Option<u64>,
    ledger: Option<u64>,
    gas: Option<u64>,
    reserve_share_ppm: Option<u32>,
    account_reserve: Option<u64>,
    hold_reserve: Option<u64>,
    starting_balance: Option<u64>,
    least_payout: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Limits {
    accounts: Option<u32>,
    holds: Option<u32>,
    holds_per_account: Option<u16>,
    withdrawals: Option<u32>,
    pending_deposits: Option<u32>,
    recent_deposits: Option<u32>,
    block_operations: Option<u16>,
    deadline_operations: Option<u16>,
    claimed_per_hold: Option<u16>,
    commitments_per_claim: Option<u16>,
    vote_window: Option<u16>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Timers {
    hold_notice: Option<u64>,
    shutdown_notice: Option<u64>,
    reconfigure_notice: Option<u64>,
    absence: Option<u64>,
    stall: Option<u64>,
    margin: Option<u64>,
    allowance: Option<u64>,
}

/// Sets `field` to `value`, if there is one.
fn set<T>(field: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *field = value;
    }
}

/// `base` with what `json` sets, checked: with a token hub's starting
/// fees first, if it names a token.
///
/// # Errors
///
/// What is wrong, for people: JSON that does not read, a field the terms
/// have not, a token that is none, or terms that do not check.
pub fn terms(mut base: Terms, json: &str) -> Result<Terms, String> {
    if json.len() > MAX_TERMS_JSON_BYTES {
        return Err(format!(
            "the terms are longer than {MAX_TERMS_JSON_BYTES} bytes"
        ));
    }
    let overrides: Overrides = if json.trim().is_empty() {
        Overrides::default()
    } else {
        serde_json::from_str(json).map_err(|error| format!("the terms do not read: {error}"))?
    };
    if let Some(token) = &overrides.asset {
        base.asset = token.asset()?;
        base.fees.ledger = 0;
        base.fees.gas = LEAST_TOKEN_GAS_DROPS;
    }
    let fees = overrides.fees.unwrap_or_default();
    let f = &mut base.fees;
    set(&mut f.operation, fees.operation);
    set(&mut f.ledger, fees.ledger);
    set(&mut f.gas, fees.gas);
    set(&mut f.reserve_share_ppm, fees.reserve_share_ppm);
    set(&mut f.account_reserve, fees.account_reserve);
    set(&mut f.hold_reserve, fees.hold_reserve);
    set(&mut f.starting_balance, fees.starting_balance);
    set(&mut f.least_payout, fees.least_payout);
    set_limits(&mut base, overrides.limits.unwrap_or_default());
    set_timers(&mut base, overrides.timers.unwrap_or_default());
    base.check()
        .map_err(|error| format!("the terms do not check: {error:?}"))?;
    Ok(base)
}

fn set_limits(base: &mut Terms, limits: Limits) {
    let l = &mut base.limits;
    set(&mut l.accounts, limits.accounts);
    set(&mut l.holds, limits.holds);
    set(&mut l.holds_per_account, limits.holds_per_account);
    set(&mut l.withdrawals, limits.withdrawals);
    set(&mut l.pending_deposits, limits.pending_deposits);
    set(&mut l.recent_deposits, limits.recent_deposits);
    set(&mut l.block_operations, limits.block_operations);
    set(&mut l.deadline_operations, limits.deadline_operations);
    set(&mut l.claimed_per_hold, limits.claimed_per_hold);
    set(&mut l.commitments_per_claim, limits.commitments_per_claim);
    set(&mut l.vote_window, limits.vote_window);
}

fn set_timers(base: &mut Terms, timers: Timers) {
    let t = &mut base.timers;
    set(&mut t.hold_notice, timers.hold_notice);
    set(&mut t.shutdown_notice, timers.shutdown_notice);
    set(&mut t.reconfigure_notice, timers.reconfigure_notice);
    set(&mut t.absence, timers.absence);
    set(&mut t.stall, timers.stall);
    set(&mut t.margin, timers.margin);
    set(&mut t.allowance, timers.allowance);
}

#[cfg(test)]
mod tests {
    use dassie_hub::founding::starting_terms;

    use super::*;

    fn base() -> Terms {
        starting_terms(
            [1; 32],
            dassie_identity::TableKeygen::identity_key(&[2; 32]).identity(),
            1,
            [3; 20],
        )
    }

    #[test]
    fn json_sets_what_it_names_and_nothing_else() {
        assert_eq!(terms(base(), "").unwrap(), base());
        let set = terms(
            base(),
            r#"{"fees":{"operation":11},"timers":{"stall":7200000}}"#,
        )
        .unwrap();
        assert_eq!(set.fees.operation, 11);
        assert_eq!(set.timers.stall, 7_200_000);
        assert_eq!(set.limits, base().limits);
        assert!(
            terms(base(), r#"{"fees":{"operaton":11}}"#).is_err(),
            "a typo"
        );
        assert!(
            terms(base(), r#"{"fees":{"operation":0}}"#)
                .unwrap_err()
                .contains("check")
        );
        assert!(terms(base(), "[").is_err());
    }

    const DSUSD: &str = r#"{"currency":"4453555344000000000000000000000000000000","issuer":"rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M"}"#;

    // A token hub's terms name the token, and start with no ledger fee and
    // the least gas in its place, which the owner may raise.
    #[test]
    fn a_token_hub_starts_with_gas_and_no_ledger_fee() {
        let token = terms(base(), &format!(r#"{{"asset":{DSUSD}}}"#)).unwrap();
        assert_eq!(
            token.asset,
            Asset::Token {
                currency: dassie_xrpl::dsusd::CURRENCY,
                issuer: dassie_xrpl::dsusd::ISSUER,
            }
        );
        assert_eq!(token.fees.ledger, 0);
        assert_eq!(token.fees.gas, LEAST_TOKEN_GAS_DROPS);
        assert_eq!(token.limits, base().limits);
        let more = terms(
            base(),
            &format!(
                r#"{{"asset":{DSUSD},"fees":{{"gas":{}}}}}"#,
                LEAST_TOKEN_GAS_DROPS + 1
            ),
        )
        .unwrap();
        assert_eq!(more.fees.gas, LEAST_TOKEN_GAS_DROPS + 1);
        // Gas in an XRP hub, or a ledger fee in a token hub, does not check.
        assert!(terms(base(), r#"{"fees":{"gas":200}}"#).is_err());
        let ledger_fee = format!(r#"{{"asset":{DSUSD},"fees":{{"ledger":340}}}}"#);
        assert!(terms(base(), &ledger_fee).is_err());
        // XRP is no token, an issuer is an address, and a typo is refused.
        for asset in [
            r#"{"currency":"XRP","issuer":"rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M"}"#,
            r#"{"currency":"DSUSD","issuer":"rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M"}"#,
            r#"{"currency":"USD","issuer":"nobody"}"#,
            r#"{"currency":"USD","isuer":"rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M"}"#,
        ] {
            let json = format!(r#"{{"asset":{asset}}}"#);
            assert!(terms(base(), &json).is_err(), "{asset}");
        }
    }
}
