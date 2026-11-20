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
use support::pair::{ca_file, key, prepare, until, until_by};
use support::rates::{RatesServer, coinbase};
use support::{Bound, START_MS, TestNode};

async fn start(dir: tempfile::TempDir, clock: ManualClock) -> TestNode {
    let env = [
        ("DASSIE_EXTRA_CA_FILE", ca_file()),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    Bound::bind_in(dir)
        .start(clock, 1, Policies::DEFAULT, &env)
        .await
}

async fn ledgers(node: &TestNode) -> proto::ListLedgersResponse {
    node.engine_started().await;
    let reply = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await;
    match reply {
        Ok(response::Method::ListLedgers(list)) => list,
        other => panic!("{other:?}"),
    }
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

/// XRP's rate from mainnet's XRP/RLUSD pool (spec/areas/xrpl.md#xrps-rate):
/// the node follows a fake mainnet's validated ledgers for the pool's price,
/// with no key and no XRPL account of its own, and has XRP's rate once the
/// window holds enough readings.
#[tokio::test(flavor = "multi_thread")]
async fn xrp_s_rate_follows_mainnet_s_pool() {
    use dassie_node::fake_ledger::{Options, Server};
    use dassie_node::store::StoredCertificate;
    use dassie_node_core::certificate::CertificateSource;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::validator_list::UnlConfig;
    use support::pair::{cert_file, key_file};

    let clock = ManualClock::new(START_MS);
    let server = Server::start(Options {
        network: &dassie_xrpl::network::MAINNET,
        listen: ([127, 0, 0, 1], 0).into(),
        faucet: Some((
            ([127, 0, 0, 1], 0).into(),
            StoredCertificate {
                source: CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(cert_file()).unwrap(),
                key_pem: std::fs::read_to_string(key_file()).unwrap(),
            },
        )),
        close_every: std::time::Duration::from_millis(100),
    })
    .await
    .unwrap();
    server.close_on(std::sync::Arc::new(clock.clone()));
    // 1.86 million XRP beside 4.65 million RLUSD: 0.4 XRP to the dollar.
    server
        .ledger()
        .set_pool(
            IssuedAsset::RLUSD,
            1_860_000_000_000,
            TokenValue::parse("4650000").unwrap(),
            300,
        )
        .unwrap();
    let unl = UnlConfig::mainnet()
        .with_override(&server.publishers().unwrap())
        .unwrap();
    let policies = Policies {
        pool_rate: dassie_xrpl::pool_rate::Timings::DEV,
        pool_rate_unl: Some(Box::leak(Box::new(unl))),
        ..Policies::DEFAULT
    };
    let env = [
        ("DASSIE_EXTRA_CA_FILE", ca_file()),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let node = Bound::bind_in(tempfile::tempdir().unwrap())
        .start(clock.clone(), 1, policies, &env)
        .await;
    prepare(&node, &key(1)).await;
    let mut owner = node.ipc().await;
    let pool = |url: &str| proto::RateSource {
        kind: "xrpl-pool".into(),
        url: url.into(),
    };
    // The node follows one pool: a second is refused.
    let two = vec![pool(&server.url), pool("wss://s1.ripple.com")];
    assert!(
        owner
            .call(Method::SetRateSources(proto::SetRateSourcesRequest {
                sources: two
            }))
            .await
            .is_err()
    );
    owner
        .call(Method::SetRateSources(proto::SetRateSourcesRequest {
            sources: vec![pool(&server.url)],
        }))
        .await
        .unwrap();
    owner
        .call(Method::SetDisplayCurrency(
            proto::SetDisplayCurrencyRequest {
                currency: "XRP".into(),
            },
        ))
        .await
        .unwrap();
    until_by(&clock, 500, async || {
        rate(&ledgers(&node).await, "XRP").is_some_and(|rate| !rate.stale)
    })
    .await;
    assert_eq!(rate(&ledgers(&node).await, "XRP").unwrap().per_usd, "0.4");
    node.stop().await;
}

/// A node whose ledgers are all in dollars asks no source, yet keeps the
/// dollar's rate fresh: its price table prices only ledgers with a fresh
/// rate, so a stale dollar would leave every one of them out.
#[tokio::test(flavor = "multi_thread")]
async fn a_dollar_only_node_keeps_the_dollar_fresh() {
    let clock = ManualClock::new(START_MS);
    let node = start(tempfile::tempdir().unwrap(), clock.clone()).await;
    prepare(&node, &key(1)).await;
    until(&clock, async || {
        rate(&ledgers(&node).await, "USD").is_some_and(|rate| !rate.stale)
    })
    .await;
    // Two hours on, past the connector's staleness bound, it is fresh still.
    for _ in 0..120 {
        clock.advance(60_000);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    until(&clock, async || {
        let usd = rate(&ledgers(&node).await, "USD").cloned();
        usd.is_some_and(|rate| {
            !rate.stale && u64::try_from(rate.as_of_unix_ms).unwrap() > START_MS + 60 * 60_000
        })
    })
    .await;
    node.stop().await;
}
