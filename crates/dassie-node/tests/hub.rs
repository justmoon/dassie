//! Settlement hubs between nodes on loopback, over real QUIC
//! (spec/areas/hubs.md#the-validator-in-the-node,
//! spec/areas/peer-protocol.md#the-hubs-stream): four validators, each with
//! its own endpoint and database, finalize blocks; a member asks them over
//! the hub's stream and trusts what f + 1 signed alike; a validator that
//! restarts goes on from what it stored; and a node that validates no such
//! hub says so.

// The validators run over real sockets, on the system's clock, or on one
// the test moves where waiting on real time would make the outcome depend
// on how busy the machine is: their timers are the production ones either
// way (spec/areas/testing.md).
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
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use dassie_hub::client::{Agreement, Request, Statement};
use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::ledger::TESTNET_NETWORK;
use dassie_hub::operation::ValidatorOperation;
use dassie_hub::rules::{Acceptance, Fees, Limits, Rules, Terms, Timers};
use dassie_hub::service::wire::Message;
use dassie_hub::{HubId, Millis};
use dassie_identity::{NodeKey, PublicKey};
use dassie_kit::ClientKey;
use dassie_node::clock::{Clock, ManualClock, SystemClock};
use dassie_node::hub::driver::Driver;
use dassie_node::hub::service::{BACKOFF_MAX, EscrowLinks, HubTask, SEND_TIMEOUT};
use dassie_node::hub::transport::{HubPermit, MAX_MEMBER_MESSAGE_BYTES, Route, StateReply};
use dassie_node::hub::{HubAccess, HubAnswer, HubClient, HubExchangeError, Hubs, Sender};
use dassie_node::outbound::Resolver;
use dassie_node::peer_server::Answer;
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::store::Store;
use dassie_node_core::connections::ConnectionLimits;
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

    fn hub_access(&self, from: &PublicKey, hub: &[u8; 32]) -> HubAccess {
        self.0.access(Some(from), hub)
    }

    fn hub_permit(&self, from: &PublicKey, access: HubAccess) -> Option<HubPermit> {
        self.0.request_permit(access, &Sender(*from))
    }

    fn hub<'a>(
        &'a self,
        from: PublicKey,
        request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<HubAnswer>> + Send + 'a>> {
        Box::pin(async move { Some(self.0.answer(Sender(from), &request).await) })
    }

    fn hub_state<'a>(
        &'a self,
        from: PublicKey,
        request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<StateReply>> + Send + 'a>> {
        Box::pin(async move {
            Some(
                self.0
                    .answer_state(Arc::clone(&self.1), &from, &request)
                    .await,
            )
        })
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

/// QUIC trusts no roots: each connection pins its node's key, and the
/// resolver only resolves names, private ones included.
fn client() -> Resolver {
    Resolver::new(true)
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
        escrow: escrow_master(sponsor, TESTNET_NETWORK, &NONCE)
            .unwrap()
            .account()
            .0,
        asset: dassie_hub::rules::Asset::Xrp,
        fees: Fees {
            operation: 10,
            ledger: 100,
            gas: 0,
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
            ValidatorKeys::derive(&node.key, TESTNET_NETWORK, &NONCE)
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
    start_on(node, rules, genesis, Arc::new(SystemClock))
}

/// [`start`], on `clock`.
fn start_on(node: &Node, rules: &Rules, genesis: Millis, clock: Arc<dyn Clock>) -> Running {
    let now = clock.now_ms();
    let driver = Driver::open(Arc::clone(&node.store), rules, genesis, &node.key, now).unwrap();
    let client = HubClient::new(Some(Arc::clone(&node.quic)), client());
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

/// How far the validators' clock moves at a time when the test moves it: a
/// quarter of a block interval, so that each of a view's waits spans several
/// polls, and a message, which crosses loopback within one, arrives within
/// them as on a working hub. One that is late on a busy machine costs its
/// view, which the hub skips and goes on from, as any hub does.
const STEP_MS: u64 = dassie_hub::consensus::Config::DEFAULT.block_interval_ms / 4;

/// The least real time between two steps, for the exchanges over the
/// sockets, which are real work.
const POLL_MS: u64 = 10;

/// The most steps a wait takes: 100 s of the hub's time, hundreds of blocks
/// however many views fail, and at least 40 s of real time, which is longer
/// than the others leave a validator that was away alone before they send to
/// it again ([`BACKOFF_MAX`]), and their exchange with it.
const STEPS: u64 = 4_000;

const _: () = assert!(
    STEPS * POLL_MS / 1_000 > BACKOFF_MAX.as_secs() + SEND_TIMEOUT.as_secs(),
    "a wait outlasts the others' backoff from a validator that comes back"
);

/// Moves `clock` a step at a time until `done`, [`STEPS`] steps at most.
async fn until_moved(clock: &ManualClock, what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..STEPS {
        if done() {
            return;
        }
        clock.advance(STEP_MS);
        tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
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
async fn a_node_that_validates_no_such_hub_resets_the_stream() {
    let shutdown = CancellationToken::new();
    let validator = node(1, &shutdown);
    let member = node(2, &shutdown);
    let member = HubClient::new(Some(Arc::clone(&member.quic)), client());
    let key = NodeKey::from_bytes(&[1; 32]).public_key();
    let message = Message::Request(Request::Account(key)).to_vec();
    let hub = HubId(dassie_hub::Digest([9; 32]));
    let to = validator.key.node_id();
    let answer = member
        .exchange(
            to,
            &validator.endpoint,
            &hub,
            &message,
            Duration::from_secs(5),
        )
        .await;
    // Nothing past the hub's ID is read: the stream is reset.
    assert!(
        matches!(answer, Err(HubExchangeError::Failed(_))),
        "{answer:?}"
    );

    // A key other than the validator's own is never reached.
    let stranger = NodeKey::from_bytes(&[3; 32]).node_id();
    let answer = member
        .exchange(
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
            // The test roots, as the other nodes of these tests trust them:
            // a test machine may have no roots of its own.
            &[
                ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
                ("DASSIE_EXTRA_CA_FILE", support::pair::CA),
            ],
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
        // The account asks again on its own timers, which run on the
        // member's clock: an exchange lost to a slow machine is asked again,
        // as in a running node.
        member.clock.advance(250);
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
// finalizes again with them. Its blocks come as fast as the test moves the
// validators' clock, not at the pace of real time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_validator_far_behind_catches_up_from_a_peers_state() {
    let shutdown = CancellationToken::new();
    let nodes: Vec<Node> = (1..=4).map(|seed| node(seed, &shutdown)).collect();
    let rules = rules(&nodes);
    let clock = ManualClock::new(support::START_MS);
    let genesis = clock.now_ms();
    let start = |node: &Node| start_on(node, &rules, genesis, Arc::new(clock.clone()));
    let mut running: Vec<Running> = nodes.iter().map(start).collect();
    until_moved(&clock, "blocks become final", || {
        nodes.iter().all(|n| height(n) >= 5)
    })
    .await;

    let away = running.remove(3);
    away.stop.cancel();
    away.task.await.unwrap();
    // Where it stopped, as it stored it: its status went with its task.
    let at = nodes[3]
        .store
        .hub(rules.id().0.as_bytes())
        .unwrap()
        .unwrap()
        .height;
    assert!(at >= 5);
    // Each keeps its last RETAINED_BLOCKS final blocks: past this, the
    // block after `at` is gone from every one of them.
    let past = at + u64::try_from(dassie_hub::consensus::RETAINED_BLOCKS).unwrap();
    until_moved(&clock, "the others finalize past what they keep", || {
        (0..3).all(|i| height(&nodes[i]) > past)
    })
    .await;

    // The others hear of it once their backoff from it ends, on real time.
    running.push(start(&nodes[3]));
    until_moved(&clock, "it catches up through a peer's state", || {
        nodes[3]
            .hubs
            .statuses()
            .first()
            .is_some_and(|s| s.caught_up > 0)
    })
    .await;
    let caught = height(&nodes[3]);
    assert!(caught > past, "it took a state from after it left");
    until_moved(&clock, "it finalizes again", || {
        height(&nodes[3]) > caught + 5
    })
    .await;

    shutdown.cancel();
    for validator in running {
        validator.stop.cancel();
        validator.task.await.unwrap();
    }
}

// spec/areas/peer-protocol.md#the-hubs-stream: before it knows a sender as
// a validator of the hub, a node reads no more than a member's largest
// request; a validator's message may be as large as any.
#[tokio::test]
async fn a_strangers_request_is_cut_at_the_member_bound() {
    let shutdown = CancellationToken::new();
    let validator = node(1, &shutdown);
    let co = node(2, &shutdown);
    let stranger = node(3, &shutdown);
    let hub = HubId(dassie_hub::Digest([9; 32]));
    let (validators, mut from_validators) = tokio::sync::mpsc::channel(4);
    let (members, mut from_members) = tokio::sync::mpsc::channel(4);
    let route = Route {
        validators,
        members,
        nodes: std::collections::BTreeSet::from([co.key.node_id()]),
    };
    validator.hubs.register(hub, route);
    let taken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&taken);
    tokio::spawn(async move {
        loop {
            let inbound = tokio::select! {
                Some(inbound) = from_validators.recv() => inbound,
                Some(inbound) = from_members.recv() => inbound,
                else => return,
            };
            counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(reply) = inbound.reply {
                let _ = reply.send(Vec::new());
            }
        }
    });
    let ask = |from: &Node, length: usize| {
        let client = HubClient::new(Some(Arc::clone(&from.quic)), client());
        let (to, endpoint) = (validator.key.node_id(), validator.endpoint.clone());
        async move {
            let message = vec![0xff; length];
            client
                .exchange(to, &endpoint, &hub, &message, Duration::from_secs(5))
                .await
        }
    };
    // A stranger's request of a member's largest size is read, and found
    // broken; one byte more is cut, and never reaches the validator.
    let at_bound = ask(&stranger, MAX_MEMBER_MESSAGE_BYTES).await;
    assert_eq!(
        at_bound,
        Err(HubExchangeError::Refused(HubAnswer::Malformed))
    );
    let past = ask(&stranger, MAX_MEMBER_MESSAGE_BYTES + 1).await;
    assert!(matches!(past, Err(HubExchangeError::Failed(_))), "{past:?}");
    // A validator of the hub is read further.
    let from_validator = ask(&co, MAX_MEMBER_MESSAGE_BYTES + 1).await;
    assert_eq!(
        from_validator,
        Err(HubExchangeError::Refused(HubAnswer::Malformed))
    );
    assert_eq!(taken.load(std::sync::atomic::Ordering::Relaxed), 0);
    shutdown.cancel();
}

/// What the owner of `node` sees of its ledgers.
async fn ledgers(node: &support::TestNode) -> Vec<dassie_node::rpc::proto::Ledger> {
    use dassie_node::rpc::proto::{self, request::Method, response};

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

/// The node's hub account, as its owner sees it.
async fn hub_status(node: &support::TestNode) -> Option<dassie_node::rpc::proto::HubStatus> {
    use dassie_node::rpc::proto::{self, request::Method, response};

    match node
        .ipc()
        .await
        .call(Method::GetHubStatus(proto::GetHubStatusRequest {}))
        .await
    {
        Ok(response::Method::GetHubStatus(reply)) => reply.status,
        _ => None,
    }
}

/// Terms of an XRP hub whose escrow is `escrow`: all a validator reads a
/// deposit to it by.
fn xrp_hub_terms(escrow: dassie_xrpl::AccountId) -> dassie_hub::rules::Terms {
    let sponsor = NodeKey::from_bytes(&[100; 32]).public_key();
    dassie_hub::founding::starting_terms([0; 32], sponsor, 1, escrow.0)
}

/// The deposits to `escrow` the fake ledger holds, as validators read them.
fn deposits(
    xrpl: &dassie_node::fake_ledger::Server,
    escrow: dassie_xrpl::AccountId,
) -> Vec<dassie_hub::operation::Deposit> {
    use dassie_xrpl::api;

    let reply = xrpl
        .ledger()
        .request(77, &api::account_tx(5, escrow, 0, u32::MAX, None));
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
            .filter_map(|record| dassie_hub::escrow::deposit_of(record, &xrp_hub_terms(escrow)))
            .collect()
    })
}

/// Adds settlement scheme `id` with `config`, once the engine takes it.
async fn add_scheme(node: &support::TestNode, id: &str, config: &str) {
    use dassie_node::rpc::proto::{self, request::Method, response};

    let mut owner = node.ipc().await;
    for _ in 0..200 {
        let reply = owner
            .call(Method::AddSettlementScheme(
                proto::AddSettlementSchemeRequest {
                    id: id.into(),
                    config: config.into(),
                },
            ))
            .await;
        if let Ok(response::Method::AddSettlementScheme(_)) = reply {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{id} is not added");
}

// spec/areas/hubs.md#in-the-node, end to end: a member node with its XRP
// Ledger testnet account on the fake ledger and the hub scheme hears from
// four validators over QUIC that it has no account, and opens one by its
// first deposit from its own XRP Ledger account: tag 0, its hub key as the
// memo. Once the validators attest it, the hub credits it, the member reads
// its account, and its books hold what the hub holds for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_node_opens_its_hub_account_by_a_deposit() {
    use dassie_node::rpc::proto;

    let shutdown = CancellationToken::new();
    let nodes: Vec<Node> = (1..=4).map(|seed| node(seed, &shutdown)).collect();
    let rules = rules(&nodes);
    let genesis = SystemClock.now_ms();
    let running: Vec<Running> = nodes.iter().map(|n| start(n, &rules, genesis)).collect();
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
    // Its HTTPS listener serves its validators' list; the member asks no
    // faucet.
    let xrpl = dassie_node::fake_ledger::Server::start(dassie_node::fake_ledger::Options {
        network: &dassie_xrpl::network::TESTNET,
        listen: "127.0.0.1:0".parse().unwrap(),
        faucet: Some((
            "127.0.0.1:0".parse().unwrap(),
            dassie_node::store::StoredCertificate {
                source: dassie_node_core::certificate::CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(support::pair::CERT).unwrap(),
                key_pem: std::fs::read_to_string(support::pair::KEY).unwrap(),
            },
        )),
        close_every: Duration::from_millis(100),
    })
    .await
    .unwrap();

    let member = support::Bound::new()
        .start(
            dassie_node::clock::ManualClock::new(support::START_MS),
            1,
            dassie_node::node::Policies::DEFAULT,
            &[
                ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
                ("DASSIE_EXTRA_CA_FILE", support::pair::CA),
            ],
        )
        .await;
    let member_key = NodeKey::from_bytes(&[60; 32]);
    support::set_up(&member, &member_key).await;
    xrpl.close_on(Arc::new(member.clock.clone()));
    let mut xrpl_config = serde_json::json!({ "server": xrpl.url, "faucet": null });
    if let (Some(config), Some(serde_json::Value::Object(publishers))) =
        (xrpl_config.as_object_mut(), xrpl.publishers())
    {
        config.extend(publishers);
    }
    support::pair::set_asset_config(
        &member,
        support::pair::XRPL_ACCOUNT,
        &xrpl_config.to_string(),
    )
    .await;
    add_scheme(&member, "xrpl-testnet-xrp", "").await;
    let address = ledgers(&member)
        .await
        .into_iter()
        .find(|l| l.id == "xrpl-testnet-xrp+xrp")
        .unwrap()
        .deposit_address;
    xrpl.ledger()
        .fund(dassie_xrpl::AccountId::parse(&address).unwrap());
    support::pair::until(&member.clock, async || {
        ledgers(&member).await.iter().any(|l| {
            l.id == "xrpl-testnet-xrp+xrp" && l.connection() == proto::LedgerConnection::Synced
        })
    })
    .await;
    let hex: String = rules.to_vec().iter().map(|b| format!("{b:02x}")).collect();
    let hub_config = serde_json::json!({ "rules": hex }).to_string();
    add_scheme(&member, dassie_hub::member::SCHEME_ID, &hub_config).await;

    // Told it has no account, the node opens one from its XRP Ledger
    // account; the validators attest the deposit they see.
    let escrow = dassie_xrpl::AccountId(rules.terms.escrow);
    support::pair::until(&member.clock, async || !deposits(&xrpl, escrow).is_empty()).await;
    let made = deposits(&xrpl, escrow);
    let hub_key = dassie_hub::escrow::member_key(&member_key, &rules.id())
        .unwrap()
        .public_key();
    assert_eq!(made.len(), 1);
    assert_eq!((made[0].tag, made[0].key), (0, Some(hub_key)));
    for validator in &running {
        let attest = ValidatorOperation::AttestDeposit(made[0]);
        validator.links.attest.send(attest).await.unwrap();
    }
    support::pair::until(&member.clock, async || {
        hub_status(&member)
            .await
            .is_some_and(|s| s.account.is_some() && s.moving.is_none() && s.balance > 0)
    })
    .await;
    let status = hub_status(&member).await.unwrap();
    let fees = rules.terms.fees;
    let units = dassie_xrpl::UNITS_PER_DROP;
    assert_eq!(status.deposited, made[0].amount * units, "what it credited");
    assert_eq!(status.reserve, fees.account_reserve * units);
    // Once the account reads its own setting of the disbursal address and
    // the state after it, the books hold what the hub holds for it.
    // The deposit less the reserve and the fee of setting the disbursal
    // address.
    let expected = (made[0].amount - fees.account_reserve - fees.operation) * units;
    support::pair::until(&member.clock, async || {
        let balance = hub_status(&member).await.map(|s| s.balance);
        let booked = ledgers(&member)
            .await
            .into_iter()
            .find(|l| l.settlement_scheme == dassie_hub::member::SCHEME_ID)
            .and_then(|l| u64::try_from(l.owner_balance).ok());
        balance == Some(expected) && booked == Some(expected)
    })
    .await;
    assert_eq!(deposits(&xrpl, escrow).len(), 1, "paid once");

    member.stop().await;
    shutdown.cancel();
    for validator in running {
        validator.stop.cancel();
        validator.task.await.unwrap();
    }
}
