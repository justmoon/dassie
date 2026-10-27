//! The rate fetcher (spec/decisions/exchange-rates.md), against a source over
//! HTTPS: the owner chooses the sources and a display currency, the node
//! fetches and keeps the rates, shows the total in that currency, marks rates
//! stale when their source stops answering, and has them again after a
//! restart.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use dassie_node::clock::ManualClock;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use support::pair::{CA, key, prepare, until, until_by};
use support::rates::{RatesServer, coinbase};
use support::{Bound, START_MS, TestNode};

async fn start(dir: tempfile::TempDir, clock: ManualClock) -> TestNode {
    let env = [
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    Bound::bind_in(dir)
        .start(clock, 1, Policies::DEFAULT, &env)
        .await
}

async fn ledgers(node: &TestNode) -> proto::ListLedgersResponse {
    let mut owner = node.ipc().await;
    for _ in 0..200 {
        let reply = owner
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await;
        match reply {
            Ok(response::Method::ListLedgers(list)) => return list,
            // The engine is still starting.
            Err(error) if error.code() == proto::ErrorCode::Unavailable => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            other => panic!("{other:?}"),
        }
    }
    panic!("the engine did not start");
}

fn rate<'a>(list: &'a proto::ListLedgersResponse, code: &str) -> Option<&'a proto::ExchangeRate> {
    list.rates.iter().find(|rate| rate.asset_code == code)
}

#[tokio::test(flavor = "multi_thread")]
async fn rates_are_fetched_kept_and_age() {
    let server = RatesServer::start(coinbase(&[("EUR", "0.5"), ("XRP", "2")])).await;
    let clock = ManualClock::new(START_MS);
    let node = start(tempfile::tempdir().unwrap(), clock.clone()).await;
    let owner_key = key(1);
    prepare(&node, &owner_key).await;
    let mut owner = node.ipc().await;
    owner
        .call(Method::StubDeposit(proto::StubDepositRequest {
            amount: 3_000_000_000,
        }))
        .await
        .unwrap();
    // Only Coinbase's format takes a URL, and only HTTPS.
    let set = |kind: &str, url: String| {
        Method::SetRateSources(proto::SetRateSourcesRequest {
            sources: vec![proto::RateSource {
                kind: kind.into(),
                url,
            }],
        })
    };
    assert!(owner.call(set("kraken", server.url())).await.is_err());
    assert!(
        owner
            .call(set("coinbase", "http://localhost/".into()))
            .await
            .is_err()
    );
    assert!(owner.call(set("forex", String::new())).await.is_err());
    owner.call(set("coinbase", server.url())).await.unwrap();
    let Ok(response::Method::GetRateSources(sources)) = owner
        .call(Method::GetRateSources(proto::GetRateSourcesRequest {}))
        .await
    else {
        panic!("no sources");
    };
    assert_eq!(sources.sources.len(), 1);
    assert_eq!(sources.sources[0].url, server.url());
    let display = |currency: &str| {
        Method::SetDisplayCurrency(proto::SetDisplayCurrencyRequest {
            currency: currency.into(),
        })
    };
    assert!(owner.call(display("XYZ")).await.is_err());
    owner.call(display("EUR")).await.unwrap();
    // Fetched: the total is what the owner has in dollars, in euros.
    until(&clock, async || ledgers(&node).await.total.is_some()).await;
    let list = ledgers(&node).await;
    assert_eq!(list.display_currency, "EUR");
    let euro = rate(&list, "EUR").unwrap().clone();
    assert_eq!((euro.per_usd.as_str(), euro.stale), ("0.5", false));
    let dollars = list.ledgers[0].owner_balance;
    let total = list.total.unwrap();
    assert_eq!(
        (total.asset_code.as_str(), total.amount),
        ("EUR", dollars / 2)
    );
    let fetched_at = euro.as_of_unix_ms;
    // The source stops answering well: the rate stays, until it is stale.
    // Half an hour on, the node asks again, and is answered badly.
    server.answer(500, String::new());
    let asked = server.asked();
    clock.advance(30 * 60 * 1_000);
    until(&clock, async || server.asked() > asked).await;
    let list = ledgers(&node).await;
    assert_eq!(rate(&list, "EUR").unwrap().as_of_unix_ms, fetched_at);
    assert!(list.total.is_some());
    clock.advance(31 * 60 * 1_000);
    until(&clock, async || ledgers(&node).await.total.is_none()).await;
    assert!(rate(&ledgers(&node).await, "EUR").unwrap().stale);
    // It answers again: the node has it at its next fetch, within a
    // minute of its clock.
    server.answer(200, coinbase(&[("EUR", "0.25")]));
    until_by(&clock, 1_000, async || ledgers(&node).await.total.is_some()).await;
    assert_eq!(ledgers(&node).await.total.unwrap().amount, dollars / 4);
    // A restart has the rates at once, though the source is silent.
    server.answer(500, String::new());
    drop(owner);
    let (dir, clock) = node.stop().await;
    let node = start(dir, clock.clone()).await;
    let list = ledgers(&node).await;
    let euro = rate(&list, "EUR").unwrap().clone();
    assert_eq!((euro.per_usd.as_str(), euro.stale), ("0.25", false));
    assert_eq!(list.total.unwrap().amount, dollars / 4);
    node.stop().await;
}
