//! The dollar tokens' constants against the live ledgers, from answers
//! captured from mainnet's and the testnet's public servers
//! (`tests/data/rlusd-and-pools.json`): RLUSD's mainnet issuer and its
//! settings as `account_info` gave them, and each network's XRP pool of its
//! dollar as `amm_info` gave it, whose second asset names the token's
//! issuer and currency as the ledger holds them
//! (spec/areas/xrpl.md#rlusd, spec/decisions/mainnet-schemes.md).

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use dassie_xrpl::amm::lp_token_currency;
use dassie_xrpl::amount::{Asset, Currency};
use dassie_xrpl::api::{account_settings_result, amm_info_result};
use dassie_xrpl::{AccountId, Token, dsusd, rlusd};
use serde_json::Value;

fn captured() -> Value {
    serde_json::from_str(include_str!("data/rlusd-and-pools.json")).unwrap()
}

fn asset(token: Token) -> Asset {
    Asset::Token {
        currency: token.currency,
        issuer: token.issuer,
    }
}

// RLUSD's mainnet issuer exists at the scheme's address, and its settings
// as mainnet's ledger gave them leave the scheme free to settle: no global
// freeze and no transfer fee. Its other settings (a destination tag, deposit
// authorisation, no incoming XRP, its master key disabled) bind only
// payments to it, which the scheme never makes.
#[test]
fn rlusds_mainnet_issuer_is_the_schemes() {
    let captured = captured();
    let answer = &captured["issuer_account_info"];
    let account = answer["account_data"]["Account"].as_str().unwrap();
    assert_eq!(AccountId::parse(account), Some(rlusd::ISSUER));
    let settings = account_settings_result(answer).unwrap();
    assert!(settings.validated);
    assert!(!settings.global_freeze());
    assert!(!settings.charges_transfer_fee());
    assert!(settings.requires_destination_tag() && settings.master_disabled());
    let obligations = captured["gateway_balances"]["obligations"]
        .as_object()
        .unwrap();
    let currencies: Vec<Option<Currency>> = obligations
        .keys()
        .map(|code| Currency::parse(code))
        .collect();
    assert_eq!(currencies, [Some(rlusd::CURRENCY)]);
}

// Each network's pool of XRP and its dollar holds the token of the
// scheme's issuer and currency, and its LP token's currency is the one the
// ledger derives from the pair: mainnet's XRP/RLUSD, the testnet's
// XRP/DSUSD, and the testnet's XRP/RLUSD of test RLUSD's issuer, whose
// scheme is disabled.
#[test]
fn each_networks_dollar_pool_holds_the_schemes_token() {
    let captured = captured();
    for (answer, token, lp) in [
        (
            "amm_info",
            rlusd::TOKEN,
            "03B245BE580EC4F4386A751C084489EC4B514A2F",
        ),
        (
            "testnet_DSUSD_amm_info",
            dsusd::TOKEN,
            "036F81D4CB3018B9752DBFB1B2072DDC3BE91D53",
        ),
        (
            "testnet_RLUSD-testnet_amm_info",
            rlusd::TESTNET_TOKEN,
            "03B245BE580EC4F4386A751C084489EC4B514A2F",
        ),
    ] {
        let info = amm_info_result(&captured[answer]).unwrap();
        assert!(info.validated, "{answer}");
        let held = info.amount2.token().unwrap();
        assert_eq!(
            (held.issuer, held.currency),
            (token.issuer, token.currency),
            "{answer}"
        );
        assert_eq!(
            info.sides(asset(token)).map(|(paid, _)| paid),
            Some(info.amount2)
        );
        let currency = Currency::parse(lp).unwrap();
        assert_eq!(info.lp_token.currency, currency, "{answer}");
        assert_eq!(
            lp_token_currency(Asset::Xrp, asset(token)),
            currency,
            "{answer}"
        );
    }
}
