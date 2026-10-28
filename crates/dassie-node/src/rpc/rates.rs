//! Exchange rates (spec/decisions/exchange-rates.md): their sources, the
//! rates the node converts at, and the owner's total in a display currency.

use std::sync::Arc;

use dassie_connector::{Rates, convert};
use dassie_ledger::Asset;

use super::handlers::Reply;
use super::proto::{self, request::Method, response};
use super::{RpcError, convert as times};
use crate::engine::{Engine, LedgerView};
use crate::node::Node;
use crate::rates::{self, MAX_SOURCES, RateSource, SourceKind, currency};

/// Rates older than this are stale: the connector's bound.
const MAX_RATE_AGE_MS: u64 = dassie_connector::Config::DEFAULT.max_rate_age_ms;

/// The scale totals are shown at.
const DISPLAY_SCALE: u8 = 9;

/// Answers the rate methods.
pub(super) async fn dispatch(node: &Arc<Node>, method: Method) -> Result<Reply, RpcError> {
    let store = node.store();
    let reply = match method {
        Method::GetRateSources(_) => {
            response::Method::GetRateSources(proto::GetRateSourcesResponse {
                sources: sources(&store.rate_sources().map_err(RpcError::internal)?),
            })
        }
        Method::SetRateSources(request) => {
            if request.sources.len() > MAX_SOURCES {
                return Err(RpcError::invalid("at most 8 rate sources"));
            }
            let chosen = request
                .sources
                .iter()
                .map(source)
                .collect::<Result<Vec<_>, _>>()?;
            store
                .set_rate_sources(&chosen)
                .map_err(RpcError::internal)?;
            node.wake_rates();
            response::Method::SetRateSources(proto::SetRateSourcesResponse {
                sources: sources(&chosen),
            })
        }
        Method::SetDisplayCurrency(request) => {
            let chosen = if request.currency.is_empty() {
                None
            } else {
                Some(
                    currency(&request.currency)
                        .ok_or_else(|| RpcError::invalid("no rates are kept for that currency"))?,
                )
            };
            store
                .set_display_currency(chosen)
                .map_err(RpcError::internal)?;
            node.wake_rates();
            if let Some(engine) = node.engine() {
                engine.touch_ledgers().await?;
            }
            response::Method::SetDisplayCurrency(proto::SetDisplayCurrencyResponse {})
        }
        _ => return Err(RpcError::internal("not a rate method")),
    };
    Ok(Reply::Unary(reply))
}

fn sources(sources: &[RateSource]) -> Vec<proto::RateSource> {
    sources
        .iter()
        .map(|source| proto::RateSource {
            kind: source.kind.as_str().into(),
            url: source.url.clone().unwrap_or_default(),
        })
        .collect()
}

fn source(message: &proto::RateSource) -> Result<RateSource, RpcError> {
    let kind = SourceKind::parse(&message.kind)
        .ok_or_else(|| RpcError::invalid("a rate source is coinbase, kraken, bitstamp or ecb"))?;
    let chosen = RateSource {
        kind,
        url: (!message.url.is_empty()).then(|| message.url.clone()),
    };
    match chosen.problem() {
        Some(problem) => Err(RpcError::invalid(problem)),
        None => Ok(chosen),
    }
}

/// The ledgers, with the rates and the owner's total.
pub(super) async fn ledgers(
    node: &Node,
    engine: &Engine,
) -> Result<proto::ListLedgersResponse, RpcError> {
    let (views, assets) = engine.ledgers_and_assets().await?;
    let rates = engine.rates().await?;
    let display = node
        .store()
        .display_currency()
        .map_err(RpcError::internal)?;
    let now = node.now_ms();
    Ok(proto::ListLedgersResponse {
        ledgers: views.iter().map(super::ledger::ledger).collect(),
        rates: rates
            .iter()
            .map(|(code, rate, as_of_ms)| proto::ExchangeRate {
                asset_code: code.into(),
                per_usd: rates::format(rate),
                as_of_unix_ms: times::time(as_of_ms),
                stale: now.saturating_sub(as_of_ms) > MAX_RATE_AGE_MS,
            })
            .collect(),
        display_currency: display.unwrap_or_default().into(),
        total: display.and_then(|code| total(&views, &rates, code, now)),
        external_assets: assets.iter().map(super::ledger::external_asset).collect(),
    })
}

/// What the owner has on every ledger, in `code`: none while a rate it
/// needs is missing or stale.
fn total(
    views: &[LedgerView],
    rates: &Rates,
    code: &'static str,
    now: u64,
) -> Option<proto::DisplayTotal> {
    let to = Asset {
        code,
        scale: DISPLAY_SCALE,
    };
    let mut sum: i128 = 0;
    for view in views {
        let from = view.id.asset();
        if !rates.fresh(from.code, code, now, MAX_RATE_AGE_MS) {
            return None;
        }
        let magnitude = u64::try_from(view.owner_balance.unsigned_abs()).ok()?;
        let converted = i128::from(convert(rates, from, to, magnitude)?);
        sum = if view.owner_balance < 0 {
            sum.checked_sub(converted)?
        } else {
            sum.checked_add(converted)?
        };
    }
    Some(proto::DisplayTotal {
        asset_code: code.into(),
        asset_scale: u32::from(DISPLAY_SCALE),
        amount: i64::try_from(sum).ok()?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use dassie_connector::RATE_PRECISION;
    use dassie_ledger::LedgerId;
    use dassie_settlement::LedgerStatus;

    use super::*;

    fn view(id: LedgerId, owner_balance: i128) -> LedgerView {
        LedgerView {
            id,
            scheme: String::new(),
            owner_balance,
            owner_pending: 0,
            on_ledger_balance: 0,
            reserve: 0,
            deposit_address: None,
            deposit_tag: None,
            issuer: None,
            faucet: false,
            peers: false,
            status: LedgerStatus::Synced,
            waiting: None,
            peg: None,
            removal_blockers: Vec::new(),
            removing: None,
        }
    }

    #[test]
    fn the_total_needs_every_rate_fresh() {
        let mut rates = Rates::new();
        let one = 10u128.pow(RATE_PRECISION);
        rates.set_at("USD", one, 1_000);
        rates.set_at("XRP", 2 * one, 1_000);
        let views = [
            view(LedgerId::StubUsd, 3_000_000_000),
            view(LedgerId::XrplTestnetXrp, -1_000_000_000),
        ];
        let total = total(&views, &rates, "USD", 1_000).unwrap();
        assert_eq!(
            (total.amount, total.asset_code.as_str()),
            (2_500_000_000, "USD")
        );
        let in_xrp = super::total(&views, &rates, "XRP", 1_000).unwrap();
        assert_eq!(in_xrp.amount, 5_000_000_000);
        assert!(super::total(&views, &rates, "USD", 1_000 + MAX_RATE_AGE_MS + 1).is_none());
        assert!(super::total(&views, &rates, "EUR", 1_000).is_none());
        // Without a rate for the euro, a dollar total still needs none.
        assert!(super::total(&views[..1], &rates, "USD", u64::MAX).is_some());
    }
}
