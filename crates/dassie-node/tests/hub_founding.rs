//! Founding a settlement hub between daemons on loopback, over real QUIC
//! (spec/areas/hubs.md#founding-a-hub, spec/areas/hubs.md#payout-bundles):
//! four validator daemons list the sponsor in their validator file; the
//! sponsor's owner founds a hub with them through the RPC; they accept,
//! sign its genesis and run it once the certificate reaches them; the
//! sponsor funds and sets up the escrow on the fake XRP Ledger from its own
//! account; the validators' watchers check it and attest it; and once the
//! hub stalls, a member fetches its validators' payout bundles and checks
//! them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::clock::ManualClock;
use dassie_node::fake_ledger::{Options, Server};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_xrpl::network::TESTNET;
use support::pair::{CA, CERT, KEY};
use support::{Bound, START_MS, TestNode, set_up};

mod support;

/// Sets `node` up with `key`, a hostname and a certificate: what a
/// validator needs to name its endpoint.
async fn prepare(node: &TestNode, key: &NodeKey) {
    set_up(node, key).await;
    let mut owner = node.ipc().await;
    let hostname = owner
        .call(Method::SetHostname(proto::SetHostnameRequest {
            hostname: "localhost".into(),
        }))
        .await;
    assert!(hostname.is_ok(), "{hostname:?}");
    let certificate = owner
        .call(Method::SetCertificate(proto::SetCertificateRequest {
            certificate_chain_pem: std::fs::read_to_string(CERT).unwrap(),
            private_key_pem: std::fs::read_to_string(KEY).unwrap(),
        }))
        .await;
    assert!(certificate.is_ok(), "{certificate:?}");
}

/// Adds settlement scheme `id` with `config`, once the engine runs.
async fn add_scheme(node: &TestNode, id: &str, config: &str) {
    node.engine_started().await;
    let added = node
        .ipc()
        .await
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: id.into(),
                config: config.into(),
            },
        ))
        .await;
    assert!(added.is_ok(), "{id} is not added: {added:?}");
}

async fn ledgers(node: &TestNode) -> Vec<proto::Ledger> {
    match node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    {
        Ok(response::Method::ListLedgers(list)) => list.ledgers,
        other => panic!("{other:?}"),
    }
}

async fn founding(node: &TestNode) -> proto::HubFounding {
    match node
        .ipc()
        .await
        .call(Method::ListHubFoundings(proto::ListHubFoundingsRequest {}))
        .await
    {
        Ok(response::Method::ListHubFoundings(list)) => list.foundings[0].clone(),
        other => panic!("{other:?}"),
    }
}

async fn validating(node: &TestNode) -> Vec<proto::HubValidator> {
    match node
        .ipc()
        .await
        .call(Method::GetStatus(proto::GetStatusRequest {}))
        .await
    {
        Ok(response::Method::GetStatus(status)) => status.validating,
        other => panic!("{other:?}"),
    }
}

/// Moves the shared clock on by `step_ms` at a time, giving the daemons
/// real time to talk, until `done` holds.
async fn until(clock: &ManualClock, step_ms: u64, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..1_500 {
        if done().await {
            return;
        }
        clock.advance(step_ms);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("not reached in time");
}

/// What every validator's file says: the sponsor it validates for, no hub
/// of its own, the fake ledger to follow escrows on, and the publisher of
/// its validators, whose UNL the escrow's ledgers are proven under.
fn validator_file(
    dir: &std::path::Path,
    sponsor: &NodeKey,
    xrpl: &str,
    publishers: Option<serde_json::Value>,
) -> String {
    let path = dir.join("validator.json");
    let mut file = serde_json::json!({
        "sponsors": [sponsor.node_id().as_str()],
        "hubs": [],
        "xrpl": xrpl,
    });
    if let (Some(file), Some(serde_json::Value::Object(publishers))) =
        (file.as_object_mut(), publishers)
    {
        file.extend(publishers);
    }
    std::fs::write(&path, file.to_string()).unwrap();
    path.to_str().unwrap().to_owned()
}

/// A fake XRP Ledger, four validator daemons whose file lists the
/// sponsor, and the sponsor's daemon, its XRP Ledger testnet account
/// funded and synced, all at one clock.
struct Net {
    xrpl: Server,
    clock: ManualClock,
    sponsor: TestNode,
    validators: Vec<TestNode>,
    keys: Vec<NodeKey>,
    _files: tempfile::TempDir,
}

impl Net {
    async fn start(before_sync: impl FnOnce(&Server)) -> Self {
        // Its HTTPS listener serves its validators' list; the sponsor asks
        // no faucet.
        let xrpl = Server::start(Options {
            network: &dassie_xrpl::network::TESTNET,
            listen: "127.0.0.1:0".parse().unwrap(),
            faucet: Some((
                "127.0.0.1:0".parse().unwrap(),
                dassie_node::store::StoredCertificate {
                    source: dassie_node_core::certificate::CertificateSource::Manual,
                    chain_pem: std::fs::read_to_string(CERT).unwrap(),
                    key_pem: std::fs::read_to_string(KEY).unwrap(),
                },
            )),
            close_every: Duration::from_millis(100),
        })
        .await
        .unwrap();
        let clock = ManualClock::new(START_MS);
        xrpl.close_on(std::sync::Arc::new(clock.clone()));
        let sponsor_key = NodeKey::from_bytes(&[70; 32]);
        let files = tempfile::tempdir().unwrap();
        let file = validator_file(files.path(), &sponsor_key, &xrpl.url, xrpl.publishers());
        let env = [
            ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
            ("DASSIE_EXTRA_CA_FILE", CA),
        ];
        let validator_env = [
            ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
            ("DASSIE_EXTRA_CA_FILE", CA),
            ("DASSIE_VALIDATOR_CONFIG", file.as_str()),
        ];
        let mut validators = Vec::new();
        let keys: Vec<NodeKey> = (71..=74)
            .map(|seed| NodeKey::from_bytes(&[seed; 32]))
            .collect();
        for (key, seed) in keys.iter().zip(71_u64..) {
            let node = Bound::new()
                .start(clock.clone(), seed, Policies::DEV, &validator_env)
                .await;
            prepare(&node, key).await;
            validators.push(node);
        }
        let sponsor = Bound::new()
            .start(clock.clone(), 70, Policies::DEV, &env)
            .await;
        prepare(&sponsor, &sponsor_key).await;
        let mut xrpl_config = serde_json::json!({ "server": xrpl.url, "faucet": null });
        if let (Some(config), Some(serde_json::Value::Object(publishers))) =
            (xrpl_config.as_object_mut(), xrpl.publishers())
        {
            config.extend(publishers);
        }
        support::pair::set_asset_config(
            &sponsor,
            support::pair::XRPL_ACCOUNT,
            &xrpl_config.to_string(),
        )
        .await;
        add_scheme(&sponsor, "xrpl-testnet-xrp", "").await;
        let address = ledgers(&sponsor)
            .await
            .into_iter()
            .find(|l| l.id == "xrpl-testnet-xrp+xrp")
            .unwrap()
            .deposit_address;
        xrpl.ledger()
            .fund(dassie_xrpl::AccountId::parse(&address).unwrap());
        before_sync(&xrpl);
        until(&clock, 250, async || {
            ledgers(&sponsor).await.iter().any(|l| {
                l.id == "xrpl-testnet-xrp+xrp" && l.connection() == proto::LedgerConnection::Synced
            })
        })
        .await;
        Self {
            xrpl,
            clock,
            sponsor,
            validators,
            keys,
            _files: files,
        }
    }

    /// The validators, as the owner invites them.
    fn invites(&self) -> Vec<proto::HubFoundingInvite> {
        self.keys
            .iter()
            .zip(&self.validators)
            .map(|(key, node)| proto::HubFoundingInvite {
                node: key.node_id().as_str().into(),
                endpoint: node.host(),
            })
            .collect()
    }

    /// Stops the sponsor's daemon and starts it again on its database.
    async fn restart_sponsor(self) -> Self {
        let (dir, clock) = self.sponsor.stop().await;
        let env = [
            ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
            ("DASSIE_EXTRA_CA_FILE", CA),
        ];
        let sponsor = Bound::bind_in(dir)
            .start(clock, 70, Policies::DEV, &env)
            .await;
        Self { sponsor, ..self }
    }

    async fn stop(self) {
        self.sponsor.stop().await;
        for node in self.validators {
            node.stop().await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sponsor_founds_a_hub_with_four_validator_daemons() {
    let net = Net::start(|_| {}).await;
    let invites = net.invites();
    let Net {
        xrpl,
        clock,
        sponsor,
        validators,
        _files,
        ..
    } = net;

    // A stranger's founding is refused; the sponsor's goes through.
    let mut owner = sponsor.ipc().await;
    let refused = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites[..3].to_vec(),
            terms: String::new(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument,
        "three validators are no set"
    );
    // An escrow funded with less than the XRP Ledger needs to create it
    // could never be set up.
    let refused = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites.clone(),
            terms: String::new(),
            escrow_funding_drops: Some(1_000),
            xrpl_server: String::new(),
        }))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument,
        "too little to create the escrow"
    );
    // The shortest stall the rules allow, so that the hub stalls within
    // the test once validators are gone.
    let terms = serde_json::json!({ "timers": { "stall": dassie_hub::consensus::MIN_STALL_MS } });
    let Ok(response::Method::FoundHub(found)) = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites.clone(),
            terms: terms.to_string(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await
    else {
        panic!("not founding");
    };
    let started = found.founding.unwrap();
    assert_eq!(started.stage(), proto::HubFoundingStage::Accepting);

    // Every validator accepts, signs, runs the hub; the escrow is funded
    // from the sponsor's account, set up, and checked by a quorum.
    until(&clock, 250, async || {
        let status = founding(&sponsor).await;
        status.stage() == proto::HubFoundingStage::Founded
    })
    .await;
    let founded = founding(&sponsor).await;
    assert!(
        founded.escrow_funded && founded.escrow_set_up,
        "{founded:?}"
    );
    assert!(founded.validators.iter().all(|v| v.accepted && v.running));
    let hub = founded.hub.clone();
    for node in &validators {
        let hubs = validating(node).await;
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].hub, hub);
    }
    until(&clock, 250, async || {
        let mut checked = 0;
        for node in &validators {
            checked += usize::from(validating(node).await.iter().all(|h| h.escrow_checked));
        }
        checked == validators.len()
    })
    .await;
    // The escrow on the ledger is the rules': its master key disabled, and
    // the validators' signer list.
    let escrow = dassie_xrpl::AccountId::parse(&founded.escrow).unwrap();
    assert!(xrpl.ledger().balance(escrow).is_some());

    // A member: the sponsor takes the hub's scheme. Two validators go, and
    // the two left declare the hub stalled once no block is final for the
    // rules' stall, and sign the payout set; the member fetches each one's
    // bundle and keeps it once it checks.
    let hub_config = serde_json::json!({ "rules": founded.rules }).to_string();
    add_scheme(&sponsor, dassie_hub::member::SCHEME_ID, &hub_config).await;
    let mut validators = validators.into_iter();
    let (first, second) = (validators.next().unwrap(), validators.next().unwrap());
    let left: Vec<TestNode> = validators.collect();
    first.stop().await;
    second.stop().await;
    // The stall is the rules' shortest, minutes on the hub's clock, which
    // moves in steps of seconds: the views it holds time out within it.
    until(&clock, 5_000, async || {
        let reply = sponsor
            .ipc()
            .await
            .call(Method::GetHubPayout(proto::GetHubPayoutRequest {}))
            .await;
        matches!(reply, Ok(response::Method::GetHubPayout(payout)) if payout.bundles.len() == 2)
    })
    .await;
    let Ok(response::Method::GetHubPayout(payout)) = sponsor
        .ipc()
        .await
        .call(Method::GetHubPayout(proto::GetHubPayoutRequest {}))
        .await
    else {
        panic!("no payout");
    };
    assert_eq!(payout.hub, hub);
    assert_eq!(payout.quorum, 3);
    assert_eq!(
        payout
            .bundles
            .iter()
            .map(|b| b.validator)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert!(payout.ready_height.is_none(), "two of three");
    for node in &left {
        assert!(validating(node).await[0].stalled);
    }

    sponsor.stop().await;
    for node in left {
        node.stop().await;
    }
}

/// A QUIC endpoint that only dials: a node of `key`'s, as a stranger or a
/// sponsor would found from.
struct Dialer;

impl dassie_node::quic::Handler for Dialer {
    fn client(&self, _address: std::net::IpAddr) -> Option<dassie_kit::ClientKey> {
        None
    }

    fn limits(&self) -> dassie_node_core::connections::ConnectionLimits {
        dassie_node_core::connections::ConnectionLimits {
            max_total: 8,
            max_per_client: 8,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: std::net::IpAddr,
        _from: dassie_identity::PublicKey,
        _body: dassie_node::quic::Body<'a>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = dassie_node::peer_server::Answer> + Send + 'a>,
    > {
        Box::pin(async {
            dassie_node::peer_server::Answer {
                status: 404,
                body: Vec::new(),
            }
        })
    }
}

/// Sends a founding request as the node of `key` to `to`, and returns the
/// answer's bytes.
async fn found_as(
    key: &NodeKey,
    to: (&NodeKey, &TestNode),
    shutdown: &tokio_util::sync::CancellationToken,
) -> Result<Vec<u8>, dassie_node::hub::HubExchangeError> {
    use dassie_oer::Encode;
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (identity, receiver) = tokio::sync::watch::channel(Some(std::sync::Arc::new(
        NodeKey::from_bytes(key.as_bytes()),
    )));
    let quic = std::sync::Arc::new(dassie_node::quic::Quic::new(socket, receiver, true).unwrap());
    tokio::spawn(std::sync::Arc::clone(&quic).serve(std::sync::Arc::new(Dialer), shutdown.clone()));
    let client = dassie_node::outbound::Resolver::new(true);
    let terms = dassie_hub::founding::starting_terms([3; 32], key.public_key(), 1, [4; 20]);
    let request = dassie_hub::founding::FoundingRequest::Propose(terms).to_vec();
    let (node, endpoint) = (to.0.node_id(), to.1.host());
    // The connection's timeout is real time, so it only guards against a
    // hang: a short one could end an exchange as unreachable on a busy
    // machine, which every check here fails.
    let answer = quic
        .hub_founding_exchange(node, &endpoint, &request, &client, support::HANG)
        .await;
    drop(identity);
    answer
}

// spec/areas/peer-protocol.md#the-hubs-stream: a founding stream from a
// node the validator's file does not list, or to a node with no validator
// file, is reset before its request is read; a listed sponsor's is read and
// answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strangers_founding_stream_is_reset_unread() {
    use dassie_node::hub::HubExchangeError;
    use dassie_oer::Decode;

    let shutdown = tokio_util::sync::CancellationToken::new();
    let clock = ManualClock::new(START_MS);
    let sponsor = NodeKey::from_bytes(&[80; 32]);
    let stranger = NodeKey::from_bytes(&[81; 32]);
    let files = tempfile::tempdir().unwrap();
    let file = validator_file(files.path(), &sponsor, "ws://127.0.0.1:1", None);
    let env = [
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_EXTRA_CA_FILE", CA),
    ];
    let validator_env = [
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_VALIDATOR_CONFIG", file.as_str()),
    ];
    let (validator_key, plain_key) = (
        NodeKey::from_bytes(&[82; 32]),
        NodeKey::from_bytes(&[83; 32]),
    );
    let validator = Bound::new()
        .start(clock.clone(), 82, Policies::DEV, &validator_env)
        .await;
    set_up(&validator, &validator_key).await;
    let plain = Bound::new()
        .start(clock.clone(), 83, Policies::DEV, &env)
        .await;
    set_up(&plain, &plain_key).await;

    for to in [(&validator_key, &validator), (&plain_key, &plain)] {
        let answer = found_as(&stranger, to, &shutdown).await;
        assert!(
            matches!(answer, Err(HubExchangeError::Failed(_))),
            "{answer:?}"
        );
    }
    let answer = found_as(&sponsor, (&plain_key, &plain), &shutdown).await;
    assert!(
        matches!(answer, Err(HubExchangeError::Failed(_))),
        "no validator file: {answer:?}"
    );
    // The listed sponsor is read and answered: the node has no hostname
    // yet to name in an acceptance.
    let answer = found_as(&sponsor, (&validator_key, &validator), &shutdown).await;
    let answer = dassie_hub::founding::FoundingAnswer::decode_exact(&answer.expect("answered"));
    assert_eq!(
        answer,
        Ok(dassie_hub::founding::FoundingAnswer::Refused(
            dassie_hub::founding::Refusal::NoEndpoint
        ))
    );

    shutdown.cancel();
    validator.stop().await;
    plain.stop().await;
}

/// The payments from `from` to `to` the fake ledger holds, each with its
/// result.
fn payments(
    xrpl: &Server,
    from: dassie_xrpl::AccountId,
    to: dassie_xrpl::AccountId,
) -> Vec<String> {
    use dassie_xrpl::api;
    let reply = xrpl
        .ledger()
        .request(77, &api::account_tx(5, from, 0, u32::MAX, None));
    let api::Message::Response {
        outcome: Ok(result),
        ..
    } = api::parse(&reply)
    else {
        return Vec::new();
    };
    api::account_tx_result(&result).map_or_else(Vec::new, |records| {
        records
            .transactions
            .iter()
            .filter(|record| record.is_payment && record.destination == Some(to))
            .map(|record| record.result.clone())
            .collect()
    })
}

// spec/areas/hubs.md#founding-a-hub: when the ledger's reserve is more
// than the sponsor funds the escrow with, the payment fails for being too
// little; the sponsor says so and pays nothing again, which would only fail
// alike and cost a fee each time.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_funding_too_little_for_the_escrow_is_paid_once() {
    // The network voted its reserve up to 5 XRP, past the 2 the sponsor
    // funds the escrow with by default.
    let net = Net::start(|xrpl| xrpl.ledger().set_reserve(5_000_000, 200_000)).await;
    let mut owner = net.sponsor.ipc().await;
    let found = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: net.invites(),
            terms: String::new(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert!(found.is_ok(), "{found:?}");
    until(&net.clock, 250, async || {
        founding(&net.sponsor).await.problem.contains("too little")
    })
    .await;
    let status = founding(&net.sponsor).await;
    let escrow = dassie_xrpl::AccountId::parse(&status.escrow).unwrap();
    let address = ledgers(&net.sponsor)
        .await
        .into_iter()
        .find(|l| l.id == "xrpl-testnet-xrp+xrp")
        .unwrap()
        .deposit_address;
    let sponsor = dassie_xrpl::AccountId::parse(&address).unwrap();
    // Until the setup, connecting again after it stopped, looked at the
    // escrow once more.
    looked_again(&net, escrow).await;
    assert_eq!(
        payments(&net.xrpl, sponsor, escrow),
        vec!["tecNO_DST_INSUF_XRP".to_string()],
        "paid once"
    );
    assert!(!founding(&net.sponsor).await.escrow_set_up);
    drop(owner);
    // Nor after a restart, when the server no longer holds the payment:
    // the founding kept why it stopped, and asks the ledger nothing.
    net.xrpl.ledger().forget_history();
    let net = net.restart_sponsor().await;
    looked_again(&net, escrow).await;
    assert!(
        payments(&net.xrpl, sponsor, escrow).is_empty(),
        "paid again after a restart"
    );
    assert!(founding(&net.sponsor).await.problem.contains("too little"));
    net.stop().await;
}

/// Runs the clock until the sponsor's escrow setup has read `escrow` from
/// the ledger again, and so decided whether to fund it, then a few steps
/// longer, for a payment it made to reach the ledger. Its callers check
/// that none did: nothing else follows a read that stops there, so there
/// is nothing to wait for, and these steps only give a wrong payment the
/// time to show. A slow machine can only let one go unseen, never fail
/// the test.
async fn looked_again(net: &Net, escrow: dassie_xrpl::AccountId) {
    let reads = net.xrpl.ledger().reads_of(escrow);
    until(&net.clock, 250, async || {
        net.xrpl.ledger().reads_of(escrow) > reads
    })
    .await;
    for _ in 0..8 {
        net.clock.advance(250);
        tokio::time::sleep(Duration::from_millis(125)).await;
    }
}

// spec/areas/hubs.md#founding-a-hub: a funding payment the XRP scheme
// cancels before it reaches the ledger moved nothing, so the sponsor pays
// again, once, and the founding goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancelled_funding_is_paid_again() {
    use dassie_xrpl::api;
    use dassie_xrpl::codec::{Amount, Payment, Signing, Transact};

    let net = Net::start(|_| {}).await;
    let address = ledgers(&net.sponsor)
        .await
        .into_iter()
        .find(|l| l.id == "xrpl-testnet-xrp+xrp")
        .unwrap()
        .deposit_address;
    let sponsor = dassie_xrpl::AccountId::parse(&address).unwrap();
    // A transaction of another wallet of the owner's takes the account's
    // next sequence behind the scheme's back, in the ledger its funding
    // payment would go into: that payment can never apply, which the
    // history proves once it tells the other, and the scheme cancels it.
    let reply = net
        .xrpl
        .ledger()
        .request(77, &api::account_info(5, sponsor));
    let api::Message::Response {
        outcome: Ok(result),
        ..
    } = api::parse(&reply)
    else {
        panic!("no account");
    };
    let sequence = api::account_info_result(&result).unwrap().sequence;
    let entropy = NodeKey::from_bytes(&[70; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let wallet = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes());
    assert_eq!(wallet.account(), sponsor);
    let other = Payment {
        account: sponsor,
        destination: dassie_xrpl::Wallet::from_entropy(&[9; 32]).account(),
        destination_tag: None,
        amount: Amount::Xrp(10 * dassie_xrpl::DROPS_PER_XRP),
        send_max: None,
        fee: dassie_xrpl::fake::FEE_BASE,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    net.xrpl.ledger().request(77, &api::submit(6, &other.blob));
    let mut owner = net.sponsor.ipc().await;
    let found = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: net.invites(),
            terms: String::new(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert!(found.is_ok(), "{found:?}");
    until(&net.clock, 250, async || {
        founding(&net.sponsor).await.stage() == proto::HubFoundingStage::Founded
    })
    .await;
    let escrow = dassie_xrpl::AccountId::parse(&founding(&net.sponsor).await.escrow).unwrap();
    assert_eq!(
        payments(&net.xrpl, sponsor, escrow),
        vec!["tesSUCCESS".to_string()],
        "the cancelled one never reached the ledger; the second paid"
    );
    drop(owner);
    net.stop().await;
}
