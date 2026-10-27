//! A payment across assets (M7c): A has dollars only, B has dollars and
//! XRP. A pays B's XRP wallet address; the quote measures the rate with a
//! probe, A pays from its dollars, and B converts what arrives at its own
//! rates. B's books show the money where it is: in dollars.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::time::Duration;

use dassie_node::fake_ledger::{Options, Server};
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node::store::StoredCertificate;
use dassie_node_core::certificate::CertificateSource;
use support::TestNode;
use support::pair::{CERT, KEY, Pair, until, until_by};
use support::rates::{RatesServer, coinbase};

const XRP_LEDGER: &str = "xrpl-testnet-xrp+xrp";
/// 2 XRP.
const RECEIVE: u64 = 2_000_000_000;

async fn fake_ledger() -> Server {
    Server::start(Options {
        network: &dassie_xrpl::network::TESTNET,
        listen: "127.0.0.1:0".parse().unwrap(),
        faucet: Some((
            "127.0.0.1:0".parse().unwrap(),
            StoredCertificate {
                source: CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(CERT).unwrap(),
                key_pem: std::fs::read_to_string(KEY).unwrap(),
            },
        )),
        close_every: Duration::from_millis(100),
    })
    .await
    .unwrap()
}

async fn ledgers(node: &TestNode) -> proto::ListLedgersResponse {
    match node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    {
        Ok(response::Method::ListLedgers(list)) => list,
        other => panic!("{other:?}"),
    }
}

async fn owner_balance(node: &TestNode, id: &str) -> i64 {
    ledgers(node)
        .await
        .ledgers
        .iter()
        .find(|ledger| ledger.id == id)
        .unwrap()
        .owner_balance
}

/// B's XRP ledger is synced, and B has a fresh rate for XRP.
async fn ready(node: &TestNode) -> bool {
    let list = ledgers(node).await;
    let synced = list.ledgers.iter().any(|ledger| {
        ledger.id == XRP_LEDGER && ledger.connection() == proto::LedgerConnection::Synced
    });
    let rated = list
        .rates
        .iter()
        .any(|rate| rate.asset_code == "XRP" && !rate.stale);
    synced && rated
}

#[tokio::test(flavor = "multi_thread")]
async fn dollars_arrive_as_xrp() {
    let server = fake_ledger().await;
    let rates = RatesServer::start(coinbase(&[("XRP", "2")])).await;
    let pair = Pair::start(51, 52).await;
    server.close_on(std::sync::Arc::new(pair.clock.clone()));
    let (a, b) = (&pair.a, &pair.b);
    let mut owner_b = b.ipc().await;
    owner_b
        .call(Method::SetRateSources(proto::SetRateSourcesRequest {
            sources: vec![proto::RateSource {
                kind: "coinbase".into(),
                url: rates.url(),
            }],
        }))
        .await
        .unwrap();
    let config = support::pair::xrpl_config(&server);
    support::pair::set_asset_config(b, support::pair::XRPL_ACCOUNT, &config).await;
    owner_b
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "xrpl-testnet-xrp".into(),
                config: String::new(),
            },
        ))
        .await
        .unwrap();
    until(&pair.clock, async || ready(b).await).await;
    let usd_a = owner_balance(a, "stub+usd").await;
    let (usd_b, xrp_b) = (
        owner_balance(b, "stub+usd").await,
        owner_balance(b, XRP_LEDGER).await,
    );

    // A quotes 2 XRP to B's XRP wallet address: paid from A's dollars,
    // the only ledger the route starts on.
    let wallet = format!("{}/pay/xrp", b.origin());
    let quoted = match a
        .ipc()
        .await
        .call(Method::QuoteSend(proto::QuoteSendRequest {
            recipient: wallet,
            ledger: String::new(),
            amount: Some(proto::quote_send_request::Amount::ReceiveAmount(RECEIVE)),
            note: "for XRP".into(),
            credentials: None,
        }))
        .await
    {
        Ok(response::Method::QuoteSend(quoted)) => quoted.quote.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(quoted.ledger, "stub+usd");
    let receive = quoted.receive_amount.clone().unwrap();
    assert_eq!(
        (receive.asset_code.as_str(), receive.value),
        ("XRP", RECEIVE)
    );
    // One dollar at two XRP each, less STREAM's 1% slippage.
    assert_eq!(quoted.debit_amount, 1_010_101_011);

    let sent = match a
        .ipc()
        .await
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: quoted.id.clone(),
        }))
        .await
    {
        Ok(response::Method::SendPayment(sent)) => sent.payment.unwrap(),
        other => panic!("{other:?}"),
    };
    let done = async || {
        let Ok(response::Method::ListOutgoingPayments(list)) = a
            .ipc()
            .await
            .call(Method::ListOutgoingPayments(
                proto::ListOutgoingPaymentsRequest { limit: 0 },
            ))
            .await
        else {
            panic!("no payments")
        };
        list.payments.into_iter().find(|p| p.id == sent.id).unwrap()
    };
    until(&pair.clock, async || done().await.state != "sending").await;
    let payment = done().await;
    assert_eq!(payment.state, "completed", "{}", payment.failure);
    // About one dollar went out; B's receiver has its 2 XRP.
    let paid = payment.sent_amount;
    assert!((1_000_000_000..=1_000_000_100).contains(&paid), "{paid}");
    // B counted each packet received as it posted it, before its Fulfill
    // went back to A (spec/decisions/own-fulfills.md).
    let received = async || {
        let Ok(response::Method::ListIncomingPayments(list)) = b
            .ipc()
            .await
            .call(Method::ListIncomingPayments(
                proto::ListIncomingPaymentsRequest { limit: 0 },
            ))
            .await
        else {
            panic!("no incoming payments")
        };
        list.payments
            .iter()
            .find(|p| quoted.receiver.ends_with(&p.id))
            .unwrap()
            .received_amount
    };
    until(&pair.clock, async || received().await >= RECEIVE).await;
    // The books: A paid its dollars; B holds them, as dollars. Its XRP
    // are as they were: what its receiver shows came through the FX
    // accounts, which are B's.
    until(&pair.clock, async || {
        owner_balance(b, "stub+usd").await == usd_b + i64::try_from(paid).unwrap()
    })
    .await;
    assert_eq!(
        owner_balance(a, "stub+usd").await,
        usd_a - i64::try_from(paid).unwrap()
    );
    assert_eq!(owner_balance(b, XRP_LEDGER).await, xrp_b);

    // B's source goes quiet for over an hour: its XRP rate is stale, and
    // B refuses what would need it with T00
    // (spec/decisions/exchange-rates.md). The rate cannot be measured, so
    // there is no quote.
    rates.answer(503, String::new());
    pair.clock.advance(61 * 60 * 1_000);
    until(&pair.clock, async || {
        ledgers(b)
            .await
            .rates
            .iter()
            .any(|rate| rate.asset_code == "XRP" && rate.stale)
    })
    .await;
    // The hour passed at once: the peers find each other again first. A
    // has heard from B since, and has a route to it.
    let prefix = format!("test.das.{}", pair.key_b.node_id());
    let peer_b = pair.key_b.node_id().to_string();
    until(&pair.clock, async || {
        let mut owner_a = a.ipc().await;
        let Ok(response::Method::ListRoutes(list)) = owner_a
            .call(Method::ListRoutes(proto::ListRoutesRequest {}))
            .await
        else {
            return false;
        };
        let Ok(response::Method::ListPeers(peers)) = owner_a
            .call(Method::ListPeers(proto::ListPeersRequest {}))
            .await
        else {
            return false;
        };
        list.routes.iter().any(|route| route.prefix == prefix)
            && peers
                .peers
                .iter()
                .any(|peer| peer.node == peer_b && peer.online)
    })
    .await;
    let seen = packets(b).await.last().and_then(|packet| packet.trace);
    let wallet = format!("{}/pay/xrp", b.origin());
    let quoting = {
        let mut owner_a = a.ipc().await;
        tokio::spawn(async move {
            owner_a
                .call(Method::QuoteSend(proto::QuoteSendRequest {
                    recipient: wallet,
                    ledger: String::new(),
                    amount: Some(proto::quote_send_request::Amount::ReceiveAmount(RECEIVE)),
                    note: String::new(),
                    credentials: None,
                }))
                .await
        })
    };
    // The probe's timers run on the nodes' clock: its volleys back off for
    // a second and a half in all. Steps of 100 ms leave the daemons real
    // time to keep hearing each other well within a peer's 60 s of
    // silence, after which its route would go.
    until_by(&pair.clock, 100, async || quoting.is_finished()).await;
    let refused = quoting.await.unwrap().unwrap_err();
    assert_eq!(refused.code(), proto::ErrorCode::Unavailable, "{refused:?}");
    let probes = packets_after(b, seen).await;
    assert!(
        probes.iter().any(|packet| packet.reject_code == "T00"),
        "{probes:?}"
    );
}

/// B's packet trace, oldest first: its last thousand packets.
async fn packets(node: &TestNode) -> Vec<proto::Packet> {
    packets_after(node, None).await
}

async fn packets_after(node: &TestNode, after: Option<proto::TraceId>) -> Vec<proto::Packet> {
    let mut all = Vec::new();
    let mut after = after;
    loop {
        let Ok(response::Method::ListPackets(page)) = node
            .ipc()
            .await
            .call(Method::ListPackets(proto::ListPacketsRequest {
                after,
                limit: 1_000,
            }))
            .await
        else {
            panic!("no packets")
        };
        let Some(last) = page.packets.last() else {
            return all;
        };
        after = last.trace;
        all.extend(page.packets);
    }
}
