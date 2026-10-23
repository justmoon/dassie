//! Settlement hubs between nodes on loopback, over real QUIC
//! (spec/areas/hubs.md#the-validator-in-the-node,
//! spec/areas/peer-protocol.md#the-hubs-stream): four validators, each with
//! its own endpoint and database, finalize blocks; a member asks them over
//! the hub's stream and trusts what f + 1 signed alike; a validator that
//! restarts goes on from what it stored; a node that validates no such
//! hub says so; and `POST /hub` takes only a member's request.

// The validators run over real sockets, so on the system's clock: their
// timers are the production ones (spec/areas/testing.md).
#![allow(
    clippy::disallowed_types,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::future::Future;
use std::net::{IpAddr, UdpSocket};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use dassie_hub::client::{Agreement, Request, Statement};
use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::operation::ValidatorOperation;
use dassie_hub::rules::{Acceptance, Fees, Limits, Rules, Terms, Timers};
use dassie_hub::service::wire::Message;
use dassie_hub::{HubId, Millis};
use dassie_identity::{NodeKey, PublicKey};
use dassie_node::clock::{Clock, SystemClock};
use dassie_node::http::peer::Answer;
use dassie_node::hub::driver::Driver;
use dassie_node::hub::service::{EscrowLinks, HubTask};
use dassie_node::hub::transport::{MEDIA_TYPE, encode_request};
use dassie_node::hub::{HubAnswer, HubClient, HubExchangeError, Hubs, Sender};
use dassie_node::peer_client::PeerClient;
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::store::Store;
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_oer::Encode;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod support;

const NONCE: [u8; 32] = [7; 32];

/// Routes the hub's stream to the hubs a node validates, as the node's own
/// handler does.
struct HubHandler(Arc<Hubs>, Arc<Store>);

impl Handler for HubHandler {
    fn client(&self, _address: IpAddr) -> Option<ClientKey> {
        None
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: 64,
            max_per_client: 64,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        _body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async {
            Answer {
                status: 404,
                body: Vec::new(),
            }
        })
    }

    fn hub<'a>(
        &'a self,
        from: PublicKey,
        request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<HubAnswer>> + Send + 'a>> {
        Box::pin(async move { Some(self.0.answer(Sender::Node(from), &request, false).await) })
    }

    fn hub_state<'a>(
        &'a self,
        from: PublicKey,
        request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move { Some(self.0.answer_state(&self.1, &from, &request)) })
    }
}

/// A node's endpoint, the hubs it validates and its database.
struct Node {
    key: Arc<NodeKey>,
    quic: Arc<Quic>,
    hubs: Arc<Hubs>,
    store: Arc<Store>,
    endpoint: String,
    // Kept alive: the endpoint presents the key it holds.
    _identity: watch::Sender<Option<Arc<NodeKey>>>,
}

fn client() -> PeerClient {
    let roots = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/pebble/root.pem"
    );
    PeerClient::new(Some(Path::new(roots)), true).unwrap()
}

fn node(seed: u8, shutdown: &CancellationToken) -> Node {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
    let (identity, receiver) = watch::channel(Some(Arc::clone(&key)));
    let quic = Arc::new(Quic::new(socket, receiver, true).unwrap());
    let hubs = Arc::new(Hubs::default());
    let store = Arc::new(Store::open_in_memory().unwrap());
    let handler = Arc::new(HubHandler(Arc::clone(&hubs), Arc::clone(&store)));
    tokio::spawn(Arc::clone(&quic).serve(handler, shutdown.clone()));
    let port = quic.local_addr().unwrap().port();
    Node {
        key,
        quic,
        hubs,
        store,
        endpoint: format!("127.0.0.1:{port}"),
        _identity: identity,
    }
}

fn terms(sponsor: &NodeKey) -> Terms {
    Terms {
        nonce: NONCE,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow: escrow_master(sponsor, &NONCE).unwrap().account().0,
        fees: Fees {
            operation: 10,
            ledger: 100,
            reserve_share_ppm: 100_000,
            account_reserve: 2_000_000,
            hold_reserve: 200_000,
            starting_balance: 1_000,
            least_payout: 1_000_000,
        },
        limits: Limits {
            accounts: 100,
            holds: 100,
            holds_per_account: 8,
            withdrawals: 16,
            pending_deposits: 64,
            recent_deposits: 256,
            block_operations: 64,
            deadline_operations: 16,
            claimed_per_hold: 1_100,
            commitments_per_claim: 16,
            vote_window: 32,
        },
        timers: Timers {
            hold_notice: 60_000,
            shutdown_notice: 60_000,
            reconfigure_notice: 120_000,
            absence: 86_400_000,
            stall: 3_600_000,
            precision: 100,
            margin: 500,
            allowance: 300,
            view_timeout: 200,
        },
    }
}

fn rules(nodes: &[Node]) -> Rules {
    let sponsor = NodeKey::from_bytes(&[100; 32]);
    let terms = terms(&sponsor);
    let validators: Vec<Acceptance> = nodes
        .iter()
        .map(|node| {
            ValidatorKeys::derive(&node.key, &NONCE)
                .unwrap()
                .accept(&node.key, &terms, node.endpoint.clone())
                .unwrap()
        })
        .collect();
    let rules = Rules { terms, validators };
    rules.check().unwrap();
    rules
}

/// A validator at work: its task, and its links to the escrow, kept open.
struct Running {
    task: JoinHandle<()>,
    stop: CancellationToken,
    links: EscrowLinks,
}

fn start(node: &Node, rules: &Rules, genesis: Millis) -> Running {
    let now = SystemClock.now_ms();
    let driver = Driver::open(Arc::clone(&node.store), rules, genesis, &node.key, now).unwrap();
    let client = HubClient::new(Some(Arc::clone(&node.quic)), client());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (task, links) = HubTask::new(driver, client, Arc::clone(&node.hubs), clock);
    let stop = CancellationToken::new();
    let stop_ = stop.clone();
    let task = tokio::spawn(async move { task.run(stop_).await.unwrap() });
    Running { task, stop, links }
}

fn height(node: &Node) -> u64 {
    node.hubs
        .statuses()
        .first()
        .map_or(0, |status| status.height)
}

/// Waits, 20 s at most, until `done`.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..2_000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("not done in time: {what}");
}

/// Asks every validator `request` at once, as a member does, until f + 1
/// of them signed a statement alike, and returns it: validators answer as
/// of their own final block, so they agree once f + 1 answer at one height.
async fn ask(member: &HubClient, nodes: &[Node], rules: &Rules, request: Request) -> Statement {
    let hub = rules.id();
    let set = rules.validator_set().unwrap();
    let mut agreement = Agreement::new(hub, set, 4096);
    let message = Message::Request(request).to_vec();
    for _ in 0..50 {
        let asks = nodes.iter().map(|node| {
            member.exchange(
                node.key.node_id(),
                &node.endpoint,
                &hub,
                &message,
                Duration::from_secs(5),
            )
        });
        for replies in futures_util::future::join_all(asks).await {
            for reply in replies.unwrap() {
                let Ok(Message::Statements(list)) = Message::decode_exact(&reply) else {
                    panic!("not statements");
                };
                if let Some(agreed) = list.into_iter().find_map(|s| agreement.add(s)) {
                    return agreed;
                }
            }
        }
    }
    panic!("no f + 1 validators agreed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_validators_finalize_blocks_over_quic_and_a_member_hears_them() {
    let shutdown = CancellationToken::new();
    let nodes: Vec<Node> = (1..=4).map(|seed| node(seed, &shutdown)).collect();
    let rules = rules(&nodes);
    let genesis = SystemClock.now_ms();
    let mut running: Vec<Running> = nodes.iter().map(|n| start(n, &rules, genesis)).collect();
    until("blocks become final", || {
        nodes.iter().all(|n| height(n) >= 10)
    })
    .await;

    // What each validator sees of the escrow reaches the ledger through
    // its attestation.
    for validator in &running {
        let attest = ValidatorOperation::AttestEscrow { sequence: 5 };
        validator.links.attest.send(attest).await.unwrap();
    }
    until("a quorum checked the escrow", || {
        nodes
            .iter()
            .all(|n| n.hubs.statuses().iter().all(|s| s.escrow_checked))
    })
    .await;

    // A member, a node of its own, asks every validator, and believes what
    // f + 1 signed alike: the newest final block each covered.
    let member = node(50, &shutdown);
    let member = HubClient::new(Some(Arc::clone(&member.quic)), client());
    let key = NodeKey::from_bytes(&[1; 32]).public_key();
    let agreed = ask(&member, &nodes, &rules, Request::Account(key)).await;
    assert!(
        matches!(agreed, Statement::Account { account: None, .. }),
        "f + 1 validators agree there is no such account: {agreed:?}"
    );

    // Validator 2 stops, and comes back from what it stored.
    let stopped = running.remove(2);
    stopped.stop.cancel();
    stopped.task.await.unwrap();
    let at = nodes[2]
        .store
        .hub(rules.id().0.as_bytes())
        .unwrap()
        .unwrap()
        .height;
    assert!(at >= 10);
    until("the others go on without it", || {
        [0, 1, 3].iter().all(|&i| height(&nodes[i]) >= at + 10)
    })
    .await;
    running.insert(2, start(&nodes[2], &rules, genesis));
    assert!(
        height(&nodes[2]) >= at,
        "it resumes from its last final block"
    );
    let others = height(&nodes[0]);
    until("it catches up and goes on", || {
        height(&nodes[2]) > others + 5
    })
    .await;
    assert!(nodes[2].hubs.statuses()[0].escrow_checked);

    shutdown.cancel();
    for validator in running {
        validator.stop.cancel();
        validator.task.await.unwrap();
    }
}

#[tokio::test]
async fn a_node_that_validates_no_such_hub_says_so() {
    let shutdown = CancellationToken::new();
    let validator = node(1, &shutdown);
    let member = node(2, &shutdown);
    let member = HubClient::new(Some(Arc::clone(&member.quic)), client());
    let key = NodeKey::from_bytes(&[1; 32]).public_key();
    let message = Message::Request(Request::Account(key)).to_vec();
    let hub = HubId(dassie_hub::Digest([9; 32]));
    let to = validator.key.node_id();
    let answer = member
        .exchange_quic(
            to,
            &validator.endpoint,
            &hub,
            &message,
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(
        answer,
        Err(HubExchangeError::Refused(HubAnswer::UnknownHub))
    );

    // A key other than the validator's own is never reached.
    let stranger = NodeKey::from_bytes(&[3; 32]).node_id();
    let answer = member
        .exchange_quic(
            stranger,
            &validator.endpoint,
            &hub,
            &message,
            Duration::from_secs(2),
        )
        .await;
    assert_eq!(answer, Err(HubExchangeError::Unreachable));
    shutdown.cancel();
}

// spec/areas/peer-protocol.md#the-hubs-stream: `POST /hub` takes only a
// member's request, and says when the node validates no such hub.
#[tokio::test]
async fn post_hub_takes_only_a_members_request() {
    let node = support::TestNode::start().await;
    let hub = HubId(dassie_hub::Digest([9; 32]));
    let post = |media: &str, message: Vec<u8>| {
        hyper::Request::post("/hub")
            .header("host", node.host())
            .header("content-type", media)
            .body(http_body_util::Full::new(bytes::Bytes::from(
                encode_request(&hub, &message),
            )))
            .unwrap()
    };
    let key = NodeKey::from_bytes(&[1; 32]).public_key();
    let request = Message::Request(Request::Account(key)).to_vec();
    let reply = support::send(node.https, true, post(MEDIA_TYPE, request.clone())).await;
    assert_eq!(reply.status, 404, "the node validates no such hub");
    let vote = Message::Consensus(vec![1]).to_vec();
    let reply = support::send(node.https, true, post(MEDIA_TYPE, vote)).await;
    assert_eq!(reply.status, 400, "validators never use HTTPS");
    let reply = support::send(node.https, true, post("application/json", request)).await;
    assert_eq!(reply.status, 415);
    node.stop().await;
}

// spec/areas/hubs.md#in-the-node: a node with the hub scheme reaches the
// hub's validators through its engine's own driver, over the hub's stream,
// and its account takes what f + 1 of them signed alike: here, that it has
// no account yet, so it waits for funds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_node_hears_its_validators_through_its_engine() {
    use dassie_node::rpc::proto::{self, request::Method, response};

    let shutdown = CancellationToken::new();
    let nodes: Vec<Node> = (1..=4).map(|seed| node(seed, &shutdown)).collect();
    let rules = rules(&nodes);
    let genesis = SystemClock.now_ms();
    let running: Vec<Running> = nodes.iter().map(|n| start(n, &rules, genesis)).collect();
    until("blocks become final", || {
        nodes.iter().all(|n| height(n) >= 3)
    })
    .await;

    let member = support::Bound::new()
        .start(
            dassie_node::clock::ManualClock::new(support::START_MS),
            1,
            dassie_node::node::Policies::DEFAULT,
            &[("DASSIE_PEER_ALLOW_PRIVATE", "1")],
        )
        .await;
    support::set_up(&member, &NodeKey::from_bytes(&[60; 32])).await;
    let mut owner = member.ipc().await;
    let hex: String = rules.to_vec().iter().map(|b| format!("{b:02x}")).collect();
    let config = serde_json::json!({ "rules": hex }).to_string();
    let mut added = None;
    for _ in 0..200 {
        let reply = owner
            .call(Method::AddSettlementScheme(
                proto::AddSettlementSchemeRequest {
                    id: dassie_hub::member::SCHEME_ID.into(),
                    config: config.clone(),
                },
            ))
            .await;
        if let Ok(response::Method::AddSettlementScheme(scheme)) = reply {
            added = scheme.ledger;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let ledger = added.expect("the hub scheme is added").id;
    let mut connection = proto::LedgerConnection::Unspecified;
    for _ in 0..1_000 {
        let Ok(response::Method::ListLedgers(list)) = owner
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await
        else {
            panic!("no ledgers");
        };
        let hub = list.ledgers.iter().find(|l| l.id == ledger).unwrap();
        connection = proto::LedgerConnection::try_from(hub.connection).unwrap();
        if connection == proto::LedgerConnection::Unfunded {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        connection,
        proto::LedgerConnection::Unfunded,
        "f + 1 validators agree the account does not exist yet"
    );
    member.stop().await;
    shutdown.cancel();
    for validator in running {
        validator.stop.cancel();
        validator.task.await.unwrap();
    }
}

// spec/areas/hubs.md#the-validator-in-the-node: a validator that was away
// longer than its peers keep blocks fetches the state after a block they
// name from one of them, on the hub's state stream, takes it, and
// finalizes again with them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_validator_far_behind_catches_up_from_a_peers_state() {
    let shutdown = CancellationToken::new();
    let nodes: Vec<Node> = (1..=4).map(|seed| node(seed, &shutdown)).collect();
    let rules = rules(&nodes);
    let genesis = SystemClock.now_ms();
    let mut running: Vec<Running> = nodes.iter().map(|n| start(n, &rules, genesis)).collect();
    until("blocks become final", || {
        nodes.iter().all(|n| height(n) >= 5)
    })
    .await;

    let away = running.remove(3);
    away.stop.cancel();
    away.task.await.unwrap();
    let at = height(&nodes[3]);
    let past = at + 2 * u64::try_from(dassie_hub::consensus::RETAINED_BLOCKS).unwrap();
    until("the others finalize past what they keep", || {
        (0..3).all(|i| height(&nodes[i]) > past)
    })
    .await;

    running.push(start(&nodes[3], &rules, genesis));
    until("it catches up through a peer's state", || {
        nodes[3]
            .hubs
            .statuses()
            .first()
            .is_some_and(|s| s.caught_up > 0)
    })
    .await;
    let caught = height(&nodes[3]);
    assert!(caught > past, "it took a state from after it left");
    until("it finalizes again", || height(&nodes[3]) > caught + 5).await;

    shutdown.cancel();
    for validator in running {
        validator.stop.cancel();
        validator.task.await.unwrap();
    }
}
