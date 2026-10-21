//! Two daemons settling on the XRP Ledger testnet scheme, over a real
//! WebSocket to the fake ledger and its faucet over HTTPS (M7b): funded by
//! the faucet, they peer, pay several times the credit, so they settle on
//! the ledger, and after both restart the books still match each account,
//! less its reserve. A deposit from outside is booked once.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::clock::ManualClock;
use dassie_node::fake_ledger::{Options, Server, close_time_now};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node::store::StoredCertificate;
use dassie_node_core::certificate::CertificateSource;
use dassie_xrpl::{AccountId, Wallet};
use support::btp_client::{BtpClient, pay_stream_waiting};
use support::pair::{CA, CERT, KEY, bootstrap, key, peered_on, prepare_with, settle, until};
use support::{Bound, START_MS, TestNode};

const LEDGER: &str = "xrpl-testnet+xrp";
/// What the faucet gives, less the reserve, in internal units.
const FUNDED: i64 = 999_000_000_000;
/// 5 XRP: over four times the credit, so it only gets through if A
/// settles with B as it goes.
const PAYMENT: u64 = 5_000_000_000;

async fn fake_ledger() -> Server {
    fake_ledger_on(0, 0).await.unwrap()
}

/// A fake ledger with its WebSocket API and faucet on these ports, or any.
async fn fake_ledger_on(port: u16, faucet_port: u16) -> Result<Server, String> {
    Server::start(Options {
        listen: ([127, 0, 0, 1], port).into(),
        faucet: Some((
            ([127, 0, 0, 1], faucet_port).into(),
            StoredCertificate {
                source: CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(CERT).unwrap(),
                key_pem: std::fs::read_to_string(KEY).unwrap(),
            },
        )),
        close_every: Duration::from_millis(100),
    })
    .await
}

/// The port at the end of a URL's authority.
fn port_of(url: &str) -> u16 {
    let authority = url.split("://").nth(1).unwrap().split('/').next().unwrap();
    authority.rsplit(':').next().unwrap().parse().unwrap()
}

fn account_of(key: &NodeKey) -> AccountId {
    let entropy = key
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    Wallet::from_entropy(entropy.as_bytes()).account()
}

async fn ledger_of(node: &TestNode) -> Option<proto::Ledger> {
    let reply = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await;
    match reply {
        Ok(response::Method::ListLedgers(list)) => {
            list.ledgers.into_iter().find(|l| l.id == LEDGER)
        }
        _ => None,
    }
}

/// Every transfer in the node's journal.
async fn journal(node: &TestNode) -> Vec<proto::Transfer> {
    let mut owner = node.ipc().await;
    let mut all = Vec::new();
    let mut after = None;
    loop {
        let Ok(response::Method::ListTransfers(page)) = owner
            .call(Method::ListTransfers(proto::ListTransfersRequest {
                after_sequence: after,
                limit: 1_000,
                account: None,
            }))
            .await
        else {
            panic!("no journal")
        };
        let Some(last) = page.transfers.last() else {
            return all;
        };
        after = Some(last.sequence);
        all.extend(page.transfers);
    }
}

/// The deposits the node booked, in order.
async fn deposits(node: &TestNode) -> Vec<u64> {
    journal(node)
        .await
        .into_iter()
        .filter(|t| t.reference == Some(proto::transfer::Reference::Other("deposit".into())))
        .map(|t| t.amount)
        .collect()
}

/// The fees the node booked, in all.
async fn fees(node: &TestNode) -> i64 {
    let total: u64 = journal(node)
        .await
        .into_iter()
        .filter(|t| matches!(t.reference, Some(proto::transfer::Reference::Fee(_))))
        .map(|t| t.amount)
        .sum();
    i64::try_from(total).unwrap()
}

/// Whether the node is up to date with the ledger, with its books matching
/// its account less the reserve.
async fn in_step(node: &TestNode, key: &NodeKey, server: &Server) -> bool {
    let Some(ledger) = ledger_of(node).await else {
        return false;
    };
    let drops = server.ledger().balance(account_of(key)).unwrap_or(0);
    ledger.connection() == proto::LedgerConnection::Synced
        && ledger.on_ledger_balance == drops * 1_000 - ledger.reserve
}

// The scheme's config, which names its server and faucet, reaches it
// through the RPC and again after a restart (RT-21, ACC-12).
#[tokio::test(flavor = "multi_thread")]
async fn two_daemons_settle_on_the_xrp_ledger_across_restarts() {
    let server = fake_ledger().await;
    let config = serde_json::json!({
        "server": server.url,
        "faucet": server.faucet_url,
    })
    .to_string();
    let (key_a, key_b) = (key(31), key(32));
    let (bound_a, bound_b) = (Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 2, Policies::DEV, &env).await;
    prepare_with(&a, &key_a, "xrpl-testnet", &config).await;
    prepare_with(&b, &key_b, "xrpl-testnet", &config).await;
    // The faucet funds both: a deposit of all but the reserve.
    until(&clock, async || {
        in_step(&a, &key_a, &server).await && in_step(&b, &key_b, &server).await
    })
    .await;
    let ledger = ledger_of(&a).await.unwrap();
    assert_eq!(ledger.owner_balance, FUNDED);
    assert_eq!(ledger.deposit_address, account_of(&key_a).to_address());
    assert_eq!(ledger.reserve, 1_000_000_000);
    until(&clock, async || {
        peered_on(&a, &key_b, LEDGER).await && peered_on(&b, &key_a, LEDGER).await
    })
    .await;
    settle(&clock).await;

    // A client of A pays B's receiver beyond the credit: A settles on the
    // ledger as it goes.
    let Ok(response::Method::CreateBtpToken(token)) = a
        .ipc()
        .await
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "wallet".into(),
            ledger: LEDGER.into(),
            cap: None,
        }))
        .await
    else {
        panic!("no token")
    };
    let credentials = b.ipc().await.incoming_payment(None).await;
    let mut client = BtpClient::connect(&a, &token.secret).await.unwrap();
    pay_stream_waiting(
        &clock,
        &mut client,
        &credentials.destination_account,
        &credentials.shared_secret,
        PAYMENT,
        Duration::from_millis(5),
    )
    .await;
    until(&clock, async || {
        in_step(&a, &key_a, &server).await && in_step(&b, &key_b, &server).await
    })
    .await;
    let settled = server.ledger().balance(account_of(&key_b)).unwrap() * 1_000 - 1_000_000_000_000;
    assert!(settled >= PAYMENT - 1_100_000_000, "settled {settled}");
    let owner_a = ledger_of(&a).await.unwrap().owner_balance;
    let owner_b = ledger_of(&b).await.unwrap().owner_balance;
    let fees_before = (fees(&a).await, fees(&b).await);

    // Both restart: they connect again, catch up, and book nothing twice.
    let (dir_a, _) = a.stop().await;
    let (dir_b, _) = b.stop().await;
    // Meanwhile, someone sends A 3 XRP.
    server.ledger().deposit(account_of(&key_a), 3_000_000);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a = Bound::bind_in(dir_a)
        .start(clock.clone(), 3, Policies::DEV, &env)
        .await;
    let b = Bound::bind_in(dir_b)
        .start(clock.clone(), 4, Policies::DEV, &env)
        .await;
    until(&clock, async || {
        in_step(&a, &key_a, &server).await && in_step(&b, &key_b, &server).await
    })
    .await;
    settle(&clock).await;
    // The deposit is booked once; the owners' funds moved only by it, and
    // by the fees of settlements made since.
    assert_eq!(deposits(&a).await, [FUNDED as u64, 3_000_000_000]);
    assert_eq!(deposits(&b).await, [FUNDED as u64]);
    let fees_a = fees(&a).await - fees_before.0;
    let fees_b = fees(&b).await - fees_before.1;
    assert_eq!(
        ledger_of(&a).await.unwrap().owner_balance,
        owner_a + 3_000_000_000 - fees_a
    );
    assert_eq!(ledger_of(&b).await.unwrap().owner_balance, owner_b - fees_b);
    a.stop().await;
    b.stop().await;
    server.stop();
}

/// A fake ledger started anew where the last one was, as the dev
/// environment does whenever it restarts, is a new chain that closes later
/// (spec/decisions/ledger-resets.md): the node sees a testnet reset, writes
/// off its old funds, and the faucet funds it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_fake_ledger_is_a_testnet_reset() {
    let server = fake_ledger().await;
    let (port, faucet_port) = (
        port_of(&server.url),
        port_of(server.faucet_url.as_deref().unwrap()),
    );
    let config = serde_json::json!({
        "server": server.url,
        "faucet": server.faucet_url,
    })
    .to_string();
    let key_a = key(33);
    let bound = Bound::new();
    let nodes = bootstrap(&key_a, bound.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound.start(clock.clone(), 1, Policies::DEV, &env).await;
    prepare_with(&a, &key_a, "xrpl-testnet", &config).await;
    until(&clock, async || in_step(&a, &key_a, &server).await).await;
    assert_eq!(deposits(&a).await, [FUNDED as u64]);

    // The old ledger had gone far on the wall clock, as in a dev session of
    // hours, and the node booked a deposit there: a new ledger would take
    // minutes to reach where the node had read to.
    let now_s = close_time_now();
    for _ in 0..2_000 {
        server.ledger().close_at(now_s);
    }
    server.ledger().deposit(account_of(&key_a), 3_000_000);
    until(&clock, async || {
        in_step(&a, &key_a, &server).await && deposits(&a).await.len() == 2
    })
    .await;
    // It goes, and a new one takes its ports.
    server.stop();
    drop(server);
    let mut server = None;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(started) = fake_ledger_on(port, faucet_port).await {
            server = Some(started);
            break;
        }
    }
    let server = server.expect("the ports came free");
    until(&clock, async || {
        in_step(&a, &key_a, &server).await && deposits(&a).await.len() == 3
    })
    .await;
    assert_eq!(
        deposits(&a).await,
        [FUNDED as u64, 3_000_000_000, FUNDED as u64]
    );
    assert_eq!(ledger_of(&a).await.unwrap().owner_balance, FUNDED);
    a.stop().await;
    server.stop();
}

/// What the fake faucet gives, in internal units.
const FAUCET: i64 = 1_000_000_000_000;

/// The owner asks the faucet for more through the RPC, at its pace: what
/// it sends is a deposit once the ledger shows it.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_asks_the_faucet_for_more() {
    let server = fake_ledger().await;
    let config = serde_json::json!({
        "server": server.url,
        "faucet": server.faucet_url,
    })
    .to_string();
    let key_a = key(34);
    let bound = Bound::new();
    let nodes = bootstrap(&key_a, bound.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound.start(clock.clone(), 1, Policies::DEV, &env).await;
    prepare_with(&a, &key_a, "xrpl-testnet", &config).await;
    until(&clock, async || in_step(&a, &key_a, &server).await).await;
    let ledger = ledger_of(&a).await.unwrap();
    assert!(ledger.faucet);
    assert_eq!(ledger.realm(), proto::Realm::Test);
    let ask = async |scheme: &str| {
        a.ipc()
            .await
            .call(Method::AskFaucet(proto::AskFaucetRequest {
                settlement_scheme: scheme.into(),
            }))
            .await
    };
    // The faucet made the account less than a minute ago.
    let early = ask("xrpl-testnet").await.unwrap_err();
    assert_eq!(early.code(), proto::ErrorCode::ResourceExhausted);
    let missing = ask("stub").await.unwrap_err();
    assert_eq!(missing.code(), proto::ErrorCode::NotFound);
    let mut asked = false;
    until(&clock, async || {
        asked = asked || ask("xrpl-testnet").await.is_ok();
        asked
    })
    .await;
    until(&clock, async || {
        in_step(&a, &key_a, &server).await && deposits(&a).await.len() == 2
    })
    .await;
    assert_eq!(deposits(&a).await, [FUNDED as u64, FAUCET as u64]);
    assert_eq!(ledger_of(&a).await.unwrap().owner_balance, FUNDED + FAUCET);
    a.stop().await;
    server.stop();
}

/// Waits, on the wall clock the fake ledger closes on, until `done`.
async fn eventually(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never happened");
}

/// The issuer of the fake ledger's test RLUSD takes requests over HTTPS
/// next to the faucet, for dev and the end-to-end tests: what it mints and
/// claws back shows on the holder's line once a ledger closes.
#[tokio::test(flavor = "multi_thread")]
async fn the_fake_issuer_mints_and_claws_back_over_https() {
    use dassie_xrpl::codec::{TF_SET_NO_RIPPLE, TokenValue, TrustSet};
    use dassie_xrpl::fake::{FEE_BASE, Token};

    let server = fake_ledger().await;
    let wallet = Wallet::from_seed([3; 16]);
    let holder = wallet.account();
    server.ledger().deposit(holder, 100_000_000);
    eventually(|| server.ledger().balance(holder).is_some()).await;
    // A new account's first sequence is the ledger that made it.
    let info = serde_json::json!({ "command": "account_info", "account": holder.to_address() });
    let info: serde_json::Value =
        serde_json::from_slice(&server.ledger().request(0, info.to_string().as_bytes())).unwrap();
    let sequence =
        u32::try_from(info["result"]["account_data"]["Sequence"].as_u64().unwrap()).unwrap();
    let ledger = server.ledger().ledger();
    let signed = TrustSet {
        account: holder,
        limit: Token::RLUSD.amount(TokenValue::MAX),
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: ledger + 20,
        flags: TF_SET_NO_RIPPLE,
        network_id: None,
        memos: Vec::new(),
        signing_pub_key: *wallet.public_key(),
        signature: None,
    }
    .sign(&wallet);
    let submit =
        serde_json::json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
    server.ledger().request(0, submit.to_string().as_bytes());
    eventually(|| {
        server
            .ledger()
            .token_balance(holder, Token::RLUSD)
            .is_some()
    })
    .await;

    let address = std::net::SocketAddr::from((
        [127, 0, 0, 1],
        port_of(server.tokens_url.as_deref().unwrap()),
    ));
    let post = |path: &str, body: serde_json::Value| {
        hyper::Request::post(path)
            .header("host", "localhost")
            .body(http_body_util::Full::new(bytes::Bytes::from(
                body.to_string(),
            )))
            .unwrap()
    };
    let mint = post(
        "/tokens/mint",
        serde_json::json!({ "destination": holder.to_address(), "value": "12.5" }),
    );
    assert_eq!(
        support::send(address, true, mint).await.status,
        hyper::StatusCode::OK
    );
    let held = |value: &str| {
        server.ledger().token_balance(holder, Token::RLUSD) == TokenValue::parse(value)
    };
    eventually(|| held("12.5")).await;
    let claw = post(
        "/tokens/clawback",
        serde_json::json!({ "account": holder.to_address(), "value": "2" }),
    );
    assert_eq!(
        support::send(address, true, claw).await.status,
        hyper::StatusCode::OK
    );
    eventually(|| held("10.5")).await;
    let refused = post(
        "/tokens/mint",
        serde_json::json!({ "destination": holder.to_address(), "value": "-1" }),
    );
    assert_eq!(
        support::send(address, true, refused).await.status,
        hyper::StatusCode::BAD_REQUEST
    );
    assert!(server.ledger().tokens_conserved());
    server.stop();
}

/// The RLUSD ledger.
const RLUSD_LEDGER: &str = "xrpl-testnet-rlusd+usd";

async fn ledger_named(node: &TestNode, id: &str) -> Option<proto::Ledger> {
    let reply = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await;
    match reply {
        Ok(response::Method::ListLedgers(list)) => list.ledgers.into_iter().find(|l| l.id == id),
        _ => None,
    }
}

/// The node's external asset, as the owner sees it.
async fn external_asset(node: &TestNode) -> Option<proto::ExternalAsset> {
    let reply = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await;
    match reply {
        Ok(response::Method::ListLedgers(list)) => list.external_assets.into_iter().next(),
        _ => None,
    }
}

/// What `key`'s account holds of RLUSD, in the ledger's internal units:
/// whole millionths.
fn rlusd_held(key: &NodeKey, server: &Server) -> Option<u64> {
    use dassie_xrpl::fake::Token;

    let held = server
        .ledger()
        .token_balance(account_of(key), Token::RLUSD)?;
    Some(u64::try_from(held.floor_units(6)?).ok()? * 1_000)
}

/// Whether the node is up to date with its RLUSD line, with its books
/// holding the line in whole millionths.
async fn rlusd_in_step(node: &TestNode, key: &NodeKey, server: &Server) -> bool {
    let Some(ledger) = ledger_named(node, RLUSD_LEDGER).await else {
        return false;
    };
    ledger.connection() == proto::LedgerConnection::Synced
        && rlusd_held(key, server).is_some_and(|held| ledger.on_ledger_balance == held)
}

// spec/areas/xrpl.md#rlusd: two daemons on `xrpl-testnet-rlusd` alone, over
// the fake ledger: the faucet gives them XRP for gas, each sets its line,
// the issuer mints test RLUSD to A, and A pays B several times the credit,
// so it settles in RLUSD as it goes. After both restart, the books still
// hold each line, and nothing is booked twice.
#[tokio::test(flavor = "multi_thread")]
async fn two_daemons_settle_rlusd_across_restarts() {
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::Token;

    let server = fake_ledger().await;
    let config = serde_json::json!({
        "server": server.url,
        "faucet": server.faucet_url,
    })
    .to_string();
    let (key_a, key_b) = (key(37), key(38));
    let (bound_a, bound_b) = (Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 2, Policies::DEV, &env).await;
    prepare_with(&a, &key_a, "xrpl-testnet-rlusd", &config).await;
    prepare_with(&b, &key_b, "xrpl-testnet-rlusd", &config).await;
    until(&clock, async || {
        rlusd_in_step(&a, &key_a, &server).await && rlusd_in_step(&b, &key_b, &server).await
    })
    .await;
    // No XRP ledger: the gas is off the books.
    assert!(ledger_of(&a).await.is_none());
    server
        .ledger()
        .issue(
            account_of(&key_a),
            Token::RLUSD.amount(TokenValue::parse("20").unwrap()),
        )
        .unwrap();
    until(&clock, async || {
        rlusd_in_step(&a, &key_a, &server).await
            && ledger_named(&a, RLUSD_LEDGER)
                .await
                .is_some_and(|ledger| ledger.owner_balance == 20_000_000_000)
    })
    .await;
    until(&clock, async || {
        peered_on(&a, &key_b, RLUSD_LEDGER).await && peered_on(&b, &key_a, RLUSD_LEDGER).await
    })
    .await;
    settle(&clock).await;

    let Ok(response::Method::CreateBtpToken(token)) = a
        .ipc()
        .await
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "wallet".into(),
            ledger: RLUSD_LEDGER.into(),
            cap: None,
        }))
        .await
    else {
        panic!("no token")
    };
    let credentials = b.ipc().await.incoming_payment(None).await;
    let mut client = BtpClient::connect(&a, &token.secret).await.unwrap();
    // 5 dollars: over four times the credit.
    pay_stream_waiting(
        &clock,
        &mut client,
        &credentials.destination_account,
        &credentials.shared_secret,
        PAYMENT,
        Duration::from_millis(5),
    )
    .await;
    until(&clock, async || {
        rlusd_in_step(&a, &key_a, &server).await && rlusd_in_step(&b, &key_b, &server).await
    })
    .await;
    let settled = rlusd_held(&key_b, &server).unwrap();
    assert!(settled >= PAYMENT - 1_100_000_000, "settled {settled}");
    let owner_a = ledger_named(&a, RLUSD_LEDGER).await.unwrap().owner_balance;
    let owner_b = ledger_named(&b, RLUSD_LEDGER).await.unwrap().owner_balance;
    // The fees were gas: no fee on the RLUSD ledger.
    assert_eq!(fees(&a).await, 0);
    // The owner sees what the account holds as the ledger says: its XRP,
    // with the reserve its line raises, and its RLUSD in millionths.
    let asset = external_asset(&a).await.unwrap();
    assert_eq!(asset.address, account_of(&key_a).to_address());
    assert_eq!(asset.settlement_schemes, ["xrpl-testnet-rlusd"]);
    let [xrp, rlusd] = asset.holdings.as_slice() else {
        panic!("{asset:?}")
    };
    assert_eq!(
        (xrp.asset_code.as_str(), xrp.asset_scale, xrp.issuer.as_str()),
        ("XRP", 6, "")
    );
    assert_eq!(Some(xrp.amount), server.ledger().balance(account_of(&key_a)));
    assert_eq!(
        xrp.reserve,
        dassie_xrpl::fake::RESERVE_BASE + dassie_xrpl::fake::RESERVE_INC
    );
    assert_eq!(
        (rlusd.asset_code.as_str(), rlusd.asset_scale, rlusd.frozen),
        ("RLUSD", 15, false)
    );
    assert_eq!(rlusd.issuer, dassie_xrpl::rlusd::ISSUER.to_address());
    assert_eq!(
        Some(rlusd.amount / 1_000_000_000 * 1_000),
        rlusd_held(&key_a, &server)
    );

    let (dir_a, _) = a.stop().await;
    let (dir_b, _) = b.stop().await;
    let a = Bound::bind_in(dir_a)
        .start(clock.clone(), 3, Policies::DEV, &env)
        .await;
    let b = Bound::bind_in(dir_b)
        .start(clock.clone(), 4, Policies::DEV, &env)
        .await;
    until(&clock, async || {
        rlusd_in_step(&a, &key_a, &server).await && rlusd_in_step(&b, &key_b, &server).await
    })
    .await;
    settle(&clock).await;
    assert_eq!(deposits(&a).await, [20_000_000_000]);
    assert!(deposits(&b).await.is_empty());
    assert_eq!(
        ledger_named(&a, RLUSD_LEDGER).await.unwrap().owner_balance,
        owner_a
    );
    assert_eq!(
        ledger_named(&b, RLUSD_LEDGER).await.unwrap().owner_balance,
        owner_b
    );
    a.stop().await;
    b.stop().await;
    server.stop();
}
