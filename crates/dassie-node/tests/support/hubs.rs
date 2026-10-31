//! Settlement hubs between daemons, for the tests of what their members do
//! (spec/areas/hubs.md#in-the-node): a fake XRP Ledger, four validator
//! daemons that run the hubs their owner's file lists, each hub's escrow set
//! up on the ledger, and member daemons that join them from their owner
//! books and peer over them, all in one runtime on tokio's paused time
//! (spec/areas/testing.md#members-of-settlement-hubs-on-paused-time).
//!
//! The daemons' clock and the fake ledger's closes read that time
//! ([`PausedClock`]), and so do the IO layer's timers. It moves only while
//! every task waits on a timer, no socket has anything to read and no
//! blocking work is under way, and then straight to the next deadline: the
//! daemons' time passes only once they are quiescent, so no message is ever
//! late however loaded the machine, and a test spends no real time waiting
//! for its clock. Each test runs on `#[tokio::test(start_paused = true)]`,
//! a runtime of one thread, which paused time needs.
//!
//! The hubs are not founded: founding is `hub_founding.rs`'s subject. A
//! validator's file names each hub's rules and genesis, as an owner's file
//! may, and the test sets each escrow up as its sponsor would, from the
//! sponsor's master key, so a test of the members starts where a founded
//! hub stands.

#![allow(clippy::panic)]

use std::collections::BTreeMap;
use std::time::Duration;

use dassie_hub::Rules;
use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::rules::Acceptance;
use dassie_identity::IdentityKey;
use dassie_node::fake_ledger::{Options, Server};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_xrpl::AccountId;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::TokenValue;
use dassie_xrpl::network::{MAINNET, Network};

use super::btp_client::{BtpClient, pay_stream};
use super::pair::{bootstrap_all, ca_file, cert_file, key_file, set_asset_config};
use super::{
    Bound, PausedClock, START_MS, STUB_UNLIMITED, TestNode, post_json, send, set_up, setup_body_in,
};

/// How often a wait looks again, in the daemons' time: a timer of the
/// test's own, so its look comes once the daemons are quiescent or one of
/// their timers falls due.
pub const STEP_MS: u64 = 250;

/// The most looks a wait takes: twenty minutes of the daemons' time, past
/// every protocol timer a member waits on, such as its look at the hub for
/// a peer's account once a minute; a wait that needs more is a hang.
const STEPS: u64 = 20 * 60 * 1_000 / STEP_MS;

/// Looks every [`STEP_MS`] of the daemons' time until `done`, [`STEPS`]
/// looks at most: a wait bounded by the daemons' time, never by real time
/// (spec/areas/testing.md#over-real-sockets).
pub async fn until(what: &str, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..STEPS {
        if done().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(STEP_MS)).await;
    }
    panic!("not reached in {STEPS} steps: {what}");
}

/// Lets `ms` of the daemons' time pass: a span in which a test checks that
/// nothing happens.
pub async fn pass(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// How long settling must stay quiet to be over: three times a member's
/// reads of its account (`ACCOUNT_EVERY_MS`), in which a settlement on its
/// way shows.
pub const QUIET_MS: u64 = 3 * dassie_hub::member::account::ACCOUNT_EVERY_MS;

/// The most rounds of [`QUIET_MS`] a wait for settling to be over takes:
/// settling that goes on through so many never ends.
const QUIET_ROUNDS: usize = 20;

/// The longest a commitment a payee took stays open after its packet
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub): a packet is held
/// at most [`MAX_HOLD_MS`](dassie_hub::rules::MAX_HOLD_MS), and its
/// commitment expires within the hub's precision of it. Within it the payee
/// claims on the hub, for a fee, whatever its payer's claims left out, and
/// after it nothing of a payment that ended is left to claim.
pub const CLAIMS_DUE_MS: u64 =
    dassie_hub::rules::MAX_HOLD_MS + dassie_hub::consensus::Config::DEFAULT.precision_ms;

/// A hub of one asset, as its terms name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubAsset {
    Xrp,
    /// The network's dollar: DSUSD on the testnet, RLUSD on mainnet.
    Dollar,
}

impl HubAsset {
    /// The terms that name it, as an owner founds with them.
    fn terms_json(self, network: &Network) -> String {
        match self {
            Self::Xrp => String::new(),
            Self::Dollar => {
                let (currency, issuer) = dollar(network);
                serde_json::json!({ "asset": {
                    "currency": currency.to_string(),
                    "issuer": issuer.to_address(),
                }})
                .to_string()
            }
        }
    }

    /// The scheme of the owner book of the asset on `network`.
    pub fn owner_book(self, network: &Network) -> &'static str {
        match (self, network.id == MAINNET.id) {
            (Self::Xrp, false) => "xrpl-testnet-xrp",
            (Self::Xrp, true) => "xrpl-xrp",
            (Self::Dollar, false) => "xrpl-testnet-dsusd",
            (Self::Dollar, true) => "xrpl-rlusd",
        }
    }

    /// The ledger of the owner book.
    pub fn owner_ledger(self, network: &Network) -> String {
        format!("{}+{}", self.owner_book(network), self.code())
    }

    /// The asset's code on the node's ledgers.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Xrp => "xrp",
            Self::Dollar => "usd",
        }
    }
}

/// The network's dollar: its currency and issuer.
fn dollar(network: &Network) -> (dassie_xrpl::codec::Currency, AccountId) {
    if network.id == MAINNET.id {
        (dassie_xrpl::rlusd::CURRENCY, dassie_xrpl::rlusd::ISSUER)
    } else {
        (dassie_xrpl::dsusd::CURRENCY, dassie_xrpl::dsusd::ISSUER)
    }
}

/// One hub: its rules, and the scheme and ledger its members join it with.
#[derive(Clone, Debug)]
pub struct Hub {
    pub rules: Rules,
    pub asset: HubAsset,
    pub scheme: &'static str,
    pub ledger: String,
}

impl Hub {
    /// The config of a member's scheme of it: the hub's rules.
    pub fn config(&self) -> String {
        serde_json::json!({ "rules": hex(&self.rules) }).to_string()
    }
}

/// The encoding of `rules`, as hex.
fn hex(rules: &Rules) -> String {
    use dassie_oer::Encode;
    rules.to_vec().iter().map(|b| format!("{b:02x}")).collect()
}

/// The key of the sponsor whose hubs the validators run: no daemon, since
/// the hubs are not founded here.
fn sponsor() -> IdentityKey {
    dassie_identity::TableKeygen::identity_key(&[70; 32])
}

/// The seeds of the validators' keys.
const VALIDATORS: std::ops::RangeInclusive<u8> = 71..=74;

/// What the escrow is funded with, in drops: as a sponsor funds it, and
/// more, so that a token hub's escrow has gas for long.
const ESCROW_DROPS: u64 = 100_000_000;

/// What a member's owner deposits to its mainnet account, which has no
/// faucet: as much as the testnet's faucet gives, 1000 XRP.
const MAINNET_DEPOSIT_DROPS: u64 = 1_000_000_000;

/// What every daemon on `network` runs with: the live realm's dev switch
/// on mainnet (spec/decisions/real-money.md).
pub fn env_on(network: &Network) -> Vec<(&'static str, String)> {
    let mut env = vec![
        ("DASSIE_PEER_ALLOW_PRIVATE", "1".to_string()),
        ("DASSIE_EXTRA_CA_FILE", ca_file().to_string()),
    ];
    if network.realm == dassie_ledger::Realm::Live {
        env.push(("DASSIE_DEV_LIVE_REALM", "1".to_string()));
    }
    env
}

/// The policies every daemon runs with: the dev ones, with production's
/// peg guard, so that a token's guard, which reads no pool here, pauses
/// none of its conversions within a test (spec/areas/xrpl.md#the-peg-guard).
pub const POLICIES: Policies = Policies {
    peg: dassie_xrpl::peg::Timings::DEFAULT,
    ..Policies::DEV
};

/// The fake ledger, the validators, and the hubs they run, on one clock.
pub struct HubNet {
    pub xrpl: Server,
    pub clock: PausedClock,
    pub network: &'static Network,
    pub hubs: Vec<Hub>,
    pub validators: Vec<TestNode>,
    _files: tempfile::TempDir,
}

/// The fake ledger of `network`, its ledgers closing on `clock` once a
/// second of it, as the XRP Ledger closes them every few, and its HTTPS
/// listener serving its faucet and its validators' list.
async fn fake_ledger(network: &'static Network, clock: PausedClock) -> Server {
    let xrpl = Server::start(Options {
        network,
        listen: "127.0.0.1:0".parse().unwrap(),
        faucet: Some((
            "127.0.0.1:0".parse().unwrap(),
            dassie_node::store::StoredCertificate {
                source: dassie_node_core::certificate::CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(cert_file()).unwrap(),
                key_pem: std::fs::read_to_string(key_file()).unwrap(),
            },
        )),
        close_every: Duration::from_secs(1),
    })
    .await
    .unwrap();
    xrpl.close_on(std::sync::Arc::new(clock));
    xrpl
}

impl HubNet {
    /// Hubs of `assets` on `network`, their escrows set up, and run by
    /// four validators that each checked every escrow.
    pub async fn start(network: &'static Network, assets: &[HubAsset]) -> Self {
        let clock = PausedClock::new();
        let xrpl = fake_ledger(network, clock).await;
        let bound: Vec<Bound> = VALIDATORS.map(|_| Bound::new()).collect();
        let keys: Vec<IdentityKey> = VALIDATORS
            .map(|seed| dassie_identity::TableKeygen::identity_key(&[seed; 32]))
            .collect();
        let hubs: Vec<Hub> = assets
            .iter()
            .zip(1_u8..)
            .map(|(asset, nonce)| {
                let endpoints = bound
                    .iter()
                    .map(|b| format!("localhost:{}", b.https_port()));
                hub(network, *asset, [nonce; 32], keys.iter().zip(endpoints))
            })
            .collect();
        for hub in &hubs {
            set_up_escrow(&xrpl, &hub.rules).await;
        }
        let files = tempfile::tempdir().unwrap();
        let file = validator_file(files.path(), network, &xrpl, &hubs);
        let mut env = env_on(network);
        env.push(("DASSIE_VALIDATOR_CONFIG", file));
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let mut validators = Vec::new();
        for ((bound, key), seed) in bound.into_iter().zip(&keys).zip(VALIDATORS) {
            let node = bound
                .start_paused(clock, u64::from(seed), POLICIES, &env)
                .await;
            set_up_in(&node, key, network).await;
            validators.push(node);
        }
        let net = Self {
            xrpl,
            clock,
            network,
            hubs,
            validators,
            _files: files,
        };
        until("every validator checked every escrow", async || {
            for node in &net.validators {
                let hubs = validating(node).await;
                if hubs.len() != net.hubs.len() || !hubs.iter().all(|h| h.escrow_checked) {
                    return false;
                }
            }
            true
        })
        .await;
        net
    }

    /// Waits until the fake ledger streams every hub's escrow to the same
    /// WebSockets, one per validator: each node follows the escrows of all
    /// the hubs it validates on one WebSocket to the server.
    pub async fn one_websocket_each(&self) {
        let escrows: Vec<AccountId> = (self.hubs.iter())
            .map(|hub| AccountId(hub.rules.terms.escrow))
            .collect();
        until("one WebSocket per validator for every escrow", async || {
            let first = self.xrpl.subscribers(escrows[0]);
            first.len() == self.validators.len()
                && (escrows.iter()).all(|escrow| self.xrpl.subscribers(*escrow) == first)
        })
        .await;
    }

    /// The hub of `asset`.
    pub fn hub(&self, asset: HubAsset) -> &Hub {
        self.hubs.iter().find(|hub| hub.asset == asset).unwrap()
    }

    /// Stops `members` and every validator, all at once.
    pub async fn stop(self, members: impl IntoIterator<Item = Member>) {
        let nodes = members
            .into_iter()
            .map(|member| member.node)
            .chain(self.validators);
        futures_util::future::join_all(nodes.map(TestNode::stop)).await;
    }

    /// Restarts `members`, all at once ([`HubNet::restart`]).
    pub async fn restart_all(&self, members: impl IntoIterator<Item = Member>) -> Vec<Member> {
        futures_util::future::join_all(members.into_iter().map(|m| self.restart(m))).await
    }
}

/// The hub of `asset` on `network` with nonce `nonce`, of `validators`,
/// each a key at its endpoint, its terms the starting ones a sponsor
/// founds with: the rules a founding would end with.
fn hub<'a>(
    network: &Network,
    asset: HubAsset,
    nonce: [u8; 32],
    validators: impl Iterator<Item = (&'a IdentityKey, String)>,
) -> Hub {
    let sponsor = sponsor();
    let master = escrow_master(&sponsor, network.id, &nonce);
    let base = dassie_hub::founding::starting_terms(
        nonce,
        sponsor.identity(),
        network.id,
        master.account().0,
    );
    let mut terms =
        dassie_node::hub::sponsor::terms::terms(base, &asset.terms_json(network)).unwrap();
    // The margin covers finality with as many validators faulty as these
    // tolerate, as a sponsor sets it.
    terms.timers.margin = terms
        .timers
        .margin
        .max(dassie_hub::rules::least_margin_ms(VALIDATORS.count()));
    let accepted: Vec<Acceptance> = validators
        .map(|(key, endpoint)| {
            ValidatorKeys::derive(key, network.id, &nonce)
                .unwrap()
                .accept(key, &terms, endpoint)
                .unwrap()
        })
        .collect();
    let rules = Rules {
        terms,
        validators: accepted,
    };
    rules.check().unwrap();
    let (scheme, _) = dassie_hub::member::scheme_of(&rules.terms).unwrap();
    let ledger = format!("{scheme}+{}", asset.code());
    Hub {
        rules,
        asset,
        scheme,
        ledger,
    }
}

/// Sets the escrow of `rules` up on the ledger, as its sponsor does: funds
/// it, then has its master key set the signer list, require a destination
/// tag, make a token hub's line and disable itself, each once a ledger
/// closed with the one before, as the test's clock moves.
async fn set_up_escrow(xrpl: &Server, rules: &Rules) {
    use dassie_xrpl::api;

    let escrow = AccountId(rules.terms.escrow);
    let master = escrow_master(&sponsor(), rules.terms.network, &rules.terms.nonce);
    xrpl.ledger().deposit(escrow, ESCROW_DROPS);
    until("the escrow funded", async || {
        xrpl.ledger().sequence(escrow).is_some()
    })
    .await;
    let sequence = xrpl.ledger().sequence(escrow).unwrap();
    let setup = dassie_hub::escrow::setup(rules, &master, sequence, dassie_xrpl::fake::FEE_BASE);
    let last = sequence + u32::try_from(setup.len()).unwrap();
    for transaction in &setup {
        xrpl.ledger()
            .request(0, &api::submit(1, transaction.blob()));
    }
    until("the escrow set up", async || {
        xrpl.ledger().sequence(escrow) == Some(last)
    })
    .await;
}

/// The validators' file: the sponsor, every hub with its genesis now, the
/// fake ledger to follow their escrows on, and the publisher of its
/// validators.
fn validator_file(dir: &std::path::Path, network: &Network, xrpl: &Server, hubs: &[Hub]) -> String {
    let path = dir.join("validator.json");
    let hubs: Vec<serde_json::Value> = hubs
        .iter()
        .map(|hub| {
            serde_json::json!({
                "rules": hex(&hub.rules),
                "genesisMs": START_MS,
            })
        })
        .collect();
    let mut file = serde_json::json!({
        "sponsors": [sponsor().node_id().as_str()],
        "hubs": hubs,
        "network": network.name,
        "xrpl": xrpl.url,
    });
    if let (Some(file), Some(serde_json::Value::Object(publishers))) =
        (file.as_object_mut(), xrpl.publishers())
    {
        file.extend(publishers);
    }
    std::fs::write(&path, file.to_string()).unwrap();
    path.to_str().unwrap().to_owned()
}

/// Sets `node` up with `key` in the realm of `network`.
async fn set_up_in(node: &TestNode, key: &IdentityKey, network: &Network) {
    if network.realm == dassie_ledger::Realm::Live {
        let token = node.setup_token().await;
        let body = setup_body_in(&token, key, "live");
        let reply = send(node.https, true, post_json(node, "/api/setup", &body)).await;
        assert_eq!(reply.status.as_u16(), 200, "{}", reply.text());
    } else {
        set_up(node, key).await;
    }
}

/// The hubs `node` validates, as its owner sees them.
pub async fn validating(node: &TestNode) -> Vec<proto::HubValidator> {
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

/// A node to start beside the hubs: the seed of its key and of its RNG,
/// the hubs it joins, whether it has dollars on the stub with unlimited
/// trust too (spec/areas/settlement.md#the-stub-scheme), what the issuer
/// sends it for a dollar hub, and the members it is pinned to as a
/// periphery node, if any (spec/areas/network.md#pinned-uplinks).
#[derive(Clone, Copy, Debug)]
pub struct Joining {
    pub seed: u8,
    pub hubs: &'static [HubAsset],
    pub stub: bool,
    /// The network's dollars the issuer sends it for a dollar hub.
    pub dollars: &'static str,
    /// The seeds of the core members it is pinned to; none for a core
    /// node.
    pub uplinks: &'static [u8],
}

impl Joining {
    /// A core member of `hubs`, sent a hundred dollars for a dollar hub.
    pub const fn of(seed: u8, hubs: &'static [HubAsset]) -> Self {
        Self {
            seed,
            hubs,
            stub: false,
            dollars: "100",
            uplinks: &[],
        }
    }

    /// With the stub too.
    pub const fn with_stub(self) -> Self {
        Self { stub: true, ..self }
    }

    /// Sent `dollars` for a dollar hub rather than a hundred.
    pub const fn with_dollars(self, dollars: &'static str) -> Self {
        Self { dollars, ..self }
    }

    /// A periphery node, set up without a hostname and pinned to the core
    /// members of seeds `uplinks`, started before it or beside it
    /// (`DASSIE_UPLINKS`): no bootstrap node lists it.
    pub const fn pinned_to(self, uplinks: &'static [u8]) -> Self {
        Self { uplinks, ..self }
    }

    /// Whether it is a periphery node.
    pub const fn periphery(&self) -> bool {
        !self.uplinks.is_empty()
    }
}

/// The stub's ledger.
pub const STUB_LEDGER: &str = "stub+usd";

/// A node started beside the hubs, a member of those it joins: its key,
/// and the hubs it is in.
pub struct Member {
    pub node: TestNode,
    pub key: IdentityKey,
    pub seed: u8,
    /// The hubs it joins, by asset.
    pub assets: Vec<HubAsset>,
    /// Whether it has the stub too.
    pub stub: bool,
    /// Whether it is a periphery node ([`Joining::pinned_to`]).
    pub periphery: bool,
    /// What it was started with, to start it again.
    env: Vec<(&'static str, String)>,
}

/// What a member's books hold of a ledger: on the ledger, and the owner's.
pub type Books = BTreeMap<String, (u64, i64)>;

impl HubNet {
    /// Nodes as `members` says, which name each other and the running
    /// members `also` as their bootstrap nodes: each joins its hubs from
    /// its owner books and peers over them. Returns once every new member's
    /// account in each of its hubs is funded.
    pub async fn members(&self, members: &[Joining], also: &[&Member]) -> Vec<Member> {
        let bound: Vec<Bound> = members.iter().map(|_| Bound::new()).collect();
        let mut named: Vec<(IdentityKey, u16)> = members
            .iter()
            .zip(&bound)
            .filter(|(joining, _)| !joining.periphery())
            .map(|(joining, bound)| {
                let key = dassie_identity::TableKeygen::identity_key(&[joining.seed; 32]);
                (key, bound.https_port())
            })
            .collect();
        named.extend(
            also.iter()
                .filter(|member| !member.periphery)
                .map(|member| {
                    let key = dassie_identity::TableKeygen::identity_key(&[member.seed; 32]);
                    (key, member.node.https.port())
                }),
        );
        let nodes = bootstrap_all(&named);
        let mut started = Vec::new();
        for (joining, bound) in members.iter().zip(bound) {
            let key = dassie_identity::TableKeygen::identity_key(&[joining.seed; 32]);
            let mut env = env_on(self.network);
            env.push(("DASSIE_BOOTSTRAP_NODES", nodes.clone()));
            if joining.periphery() {
                let uplinks: Vec<String> = joining
                    .uplinks
                    .iter()
                    .map(|seed| {
                        dassie_identity::TableKeygen::identity_key(&[*seed; 32])
                            .node_id()
                            .to_string()
                    })
                    .collect();
                env.push(("DASSIE_UPLINKS", uplinks.join(",")));
            }
            let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let node = bound
                .start_paused(self.clock, u64::from(joining.seed), POLICIES, &pairs)
                .await;
            let member = Member {
                node,
                key,
                seed: joining.seed,
                assets: joining.hubs.to_vec(),
                stub: joining.stub,
                periphery: joining.periphery(),
                env,
            };
            self.join(&member, joining.dollars).await;
            started.push(member);
        }
        for member in &started {
            until("the member funded in its hubs", async || {
                self.funded(member).await
            })
            .await;
        }
        started
    }

    /// Sets `member` up as a peer, at a hostname unless it is a periphery
    /// node, with the stub if it has it, and has it join its hubs: an owner
    /// book of each hub's asset on its one XRP Ledger account, funded by
    /// the faucet on the testnet and by the owner's deposit on mainnet,
    /// `dollars` of the network's dollars from its issuer for a dollar hub,
    /// and each hub's scheme, which opens its account in the hub by a
    /// deposit by itself.
    async fn join(&self, member: &Member, dollars: &str) {
        let node = &member.node;
        set_up_in(node, &member.key, self.network).await;
        if !member.periphery {
            let mut owner = node.ipc().await;
            let hostname = owner
                .call(Method::SetHostname(proto::SetHostnameRequest {
                    hostname: "localhost".into(),
                }))
                .await;
            assert!(hostname.is_ok(), "{hostname:?}");
            let certificate = owner
                .call(Method::SetCertificate(proto::SetCertificateRequest {
                    certificate_chain_pem: std::fs::read_to_string(cert_file()).unwrap(),
                    private_key_pem: std::fs::read_to_string(key_file()).unwrap(),
                }))
                .await;
            assert!(certificate.is_ok(), "{certificate:?}");
        }
        if member.stub {
            add_scheme(node, "stub", STUB_UNLIMITED).await;
        }
        if member.assets.is_empty() {
            return;
        }
        // On the testnet, the account asks the fake ledger's faucet for its
        // XRP; mainnet has none, and the owner deposits.
        let mut config =
            serde_json::json!({ "server": self.xrpl.url, "faucet": self.xrpl.faucet_url });
        if let (Some(config), Some(serde_json::Value::Object(publishers))) =
            (config.as_object_mut(), self.xrpl.publishers())
        {
            config.extend(publishers);
        }
        set_asset_config(node, &self.network.first().to_string(), &config.to_string()).await;
        // The XRP book, which pays a token's gas too.
        add_scheme(node, HubAsset::Xrp.owner_book(self.network), "").await;
        if member.assets.contains(&HubAsset::Dollar) {
            add_scheme(node, HubAsset::Dollar.owner_book(self.network), "").await;
        }
        let account = account_of(&member.key, self.network);
        if self.network.faucet.is_none() {
            self.xrpl.ledger().deposit(account, MAINNET_DEPOSIT_DROPS);
        }
        if member.assets.contains(&HubAsset::Dollar) {
            // Once its line is set, the issuer sends it its dollars.
            let (currency, issuer) = dollar(self.network);
            until("the member's line", async || {
                self.xrpl.ledger().line(account, issuer, currency).is_some()
            })
            .await;
            let token = IssuedAsset { currency, issuer };
            self.xrpl
                .ledger()
                .issue(account, token.amount(TokenValue::parse(dollars).unwrap()))
                .unwrap();
        }
        for asset in &member.assets {
            let hub = self.hub(*asset);
            add_scheme(node, hub.scheme, &hub.config()).await;
        }
    }

    /// Whether `member` follows each of its hubs and has an account in it
    /// with funds.
    pub async fn funded(&self, member: &Member) -> bool {
        for asset in &member.assets {
            let Some(view) = ledger(&member.node, &self.hub(*asset).ledger).await else {
                return false;
            };
            if view.connection() != proto::LedgerConnection::Synced || view.owner_balance <= 0 {
                return false;
            }
        }
        true
    }

    /// Stops `member` and starts it again on its database and its ports, as
    /// a node restarts.
    pub async fn restart(&self, member: Member) -> Member {
        let Member {
            node,
            key,
            seed,
            assets,
            stub,
            periphery,
            env,
        } = member;
        let ports = node.ports();
        let (dir, _) = node.stop().await;
        let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let node = Bound::bind_again(dir, ports)
            .await
            .start_paused(self.clock, u64::from(seed) + 100, POLICIES, &pairs)
            .await;
        node.engine_started().await;
        Member {
            node,
            key,
            seed,
            assets,
            stub,
            periphery,
            env,
        }
    }

    /// The ledgers of a member's hubs: each hub's, the owner book of each
    /// hub's asset, and the owner book of XRP, which pays a token's gas.
    fn ledgers_of(&self, member: &Member) -> Vec<String> {
        let mut ids = Vec::new();
        if !member.assets.is_empty() {
            ids.push(HubAsset::Xrp.owner_ledger(self.network));
        }
        for asset in &member.assets {
            ids.push(asset.owner_ledger(self.network));
            ids.push(self.hub(*asset).ledger.clone());
        }
        ids.sort();
        ids.dedup();
        ids
    }

    /// What `member`'s books hold of each of its ledgers: those of its
    /// hubs, and the stub's.
    pub async fn books(&self, member: &Member) -> Books {
        let mut books = Books::new();
        let stub = member.stub.then(|| STUB_LEDGER.to_owned());
        for id in self.ledgers_of(member).into_iter().chain(stub) {
            let view = ledger(&member.node, &id).await.unwrap();
            books.insert(id, (view.on_ledger_balance, view.owner_balance));
        }
        books
    }

    /// Whether `member` is quiet and its books add up, by its own books
    /// alone (spec/areas/testing.md#each-node-judged-by-its-own-books):
    /// every ledger synced, nothing on its way to a peer, no move between a
    /// hub and its owner book on its way, no settling over a hub under way
    /// (`HubStatus.settling`), and nothing the owner has pending; each owner book holding what its XRP Ledger
    /// account holds of its asset, less the reserve; and each hub's book
    /// what its account in the hub holds and its peers' holds toward it, as
    /// the hub's log shows them to it, since it booked their settlements as
    /// they funded them.
    pub async fn in_step(&self, member: &Member) -> bool {
        let node = &member.node;
        if channels(node).await.iter().any(|channel| channel.sending) {
            return false;
        }
        let list = list_ledgers(node).await;
        let statuses = hub_statuses(node).await;
        // A move between a hub and its owner book has left the one side's
        // books and not yet reached the other's; and settling over a hub
        // under way, as the node reports it, which the end-to-end tests
        // wait on alone.
        if statuses
            .iter()
            .any(|status| status.moving.is_some() || status.settling)
        {
            return false;
        }
        for id in self.ledgers_of(member) {
            let Some(view) = list.ledgers.iter().find(|l| l.id == id) else {
                return false;
            };
            if view.connection() != proto::LedgerConnection::Synced || view.owner_pending != 0 {
                return false;
            }
            let held = held(&list, &view.settlement_scheme);
            let expected = match statuses.iter().find(|s| s.scheme == view.settlement_scheme) {
                Some(status) => held
                    .map(|held| held + status.taking.iter().map(|hold| hold.amount).sum::<u64>()),
                None => held,
            };
            if expected != Some(view.on_ledger_balance) {
                return false;
            }
        }
        true
    }

    /// Waits until every claim of the payments that ended was made
    /// ([`CLAIMS_DUE_MS`]) and settling is over ([`HubNet::settled`]):
    /// nothing a member does after it, such as a restart, finds a claim
    /// still to make and its fee still to book. Returns their books.
    pub async fn claimed(&self, members: &[&Member]) -> Vec<Books> {
        pass(CLAIMS_DUE_MS).await;
        self.settled(members).await
    }

    /// Waits until every member is in step and stays so, its books
    /// unchanged, for [`QUIET_MS`] of the daemons' time more: settling over
    /// the hubs is over. Returns their books.
    pub async fn settled(&self, members: &[&Member]) -> Vec<Books> {
        for _ in 0..QUIET_ROUNDS {
            until("every member in step", async || {
                for member in members {
                    if !self.in_step(member).await {
                        return false;
                    }
                }
                true
            })
            .await;
            let mut first = Vec::new();
            for member in members {
                first.push(self.books(member).await);
            }
            pass(QUIET_MS).await;
            let mut again = Vec::new();
            let mut all = true;
            for member in members {
                all &= self.in_step(member).await;
                again.push(self.books(member).await);
            }
            if all && again == first {
                return first;
            }
        }
        panic!("settling over the hubs went on for {QUIET_ROUNDS} rounds");
    }

    /// Waits until each two of `members` that share a hub peer, with a
    /// peer account on the ledger of each hub both are in and on no other,
    /// and each holds funds toward the other in each, so either can pay
    /// the other over it; and that every member without the stub peers
    /// over hubs alone.
    pub async fn peered(&self, members: &[&Member]) {
        until("peered over every hub both are in", async || {
            for member in members {
                for peer in members {
                    let shared = self.shared(member, peer);
                    if member.key.node_id() == peer.key.node_id() || shared.is_empty() {
                        continue;
                    }
                    if peer_ledgers(&member.node, &peer.key).await != shared {
                        return false;
                    }
                    let statuses = hub_statuses(&member.node).await;
                    let peer_id = peer.key.node_id().to_string();
                    for hub in &shared {
                        let scheme = self.hubs.iter().find(|h| &h.ledger == hub).unwrap().scheme;
                        let holds = statuses
                            .iter()
                            .find(|s| s.scheme == scheme)
                            .is_some_and(|s| {
                                s.paying
                                    .iter()
                                    .any(|hold| hold.peer == peer_id && hold.amount > 0)
                            });
                        if !holds {
                            return false;
                        }
                    }
                }
            }
            true
        })
        .await;
        for member in members.iter().filter(|member| !member.stub) {
            for peer in peers(&member.node).await {
                for method in &peer.methods {
                    assert!(
                        self.hubs.iter().any(|hub| hub.scheme == method.scheme),
                        "{} peers over {}",
                        member.seed,
                        method.scheme
                    );
                }
            }
        }
    }

    /// The ledgers of the hubs `a` and `b` are both in, sorted.
    pub fn shared(&self, a: &Member, b: &Member) -> Vec<String> {
        let mut shared: Vec<String> = a
            .assets
            .iter()
            .filter(|asset| b.assets.contains(asset))
            .map(|asset| self.hub(*asset).ledger.clone())
            .collect();
        shared.sort();
        shared
    }
}

/// The XRP Ledger account of the node of `key` on `network`, its first.
pub fn account_of(key: &IdentityKey, network: &Network) -> AccountId {
    let entropy = key.xrpl_account_entropy(network.id, 0);
    dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account()
}

/// Adds settlement scheme `id` with `config`, once the engine runs.
pub async fn add_scheme(node: &TestNode, id: &str, config: &str) {
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

/// The node's ledgers, external assets and rates, as its owner sees them.
pub async fn list_ledgers(node: &TestNode) -> proto::ListLedgersResponse {
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

/// The node's ledger `id`, if it has one.
pub async fn ledger(node: &TestNode, id: &str) -> Option<proto::Ledger> {
    list_ledgers(node)
        .await
        .ledgers
        .into_iter()
        .find(|ledger| ledger.id == id)
}

/// What the external asset that `scheme` settles through holds, less what
/// its ledger holds back, in the units of the scheme's ledger: of its one
/// asset in a hub, else of XRP or the token, in whole millionths at scale
/// 9.
fn held(list: &proto::ListLedgersResponse, scheme: &str) -> Option<u64> {
    let asset = list
        .external_assets
        .iter()
        .find(|asset| asset.settlement_schemes.iter().any(|s| s == scheme))?;
    let holding = if asset.holdings.len() == 1 {
        asset.holdings.first()
    } else {
        let code = if scheme.ends_with("-xrp") { "XRP" } else { "" };
        asset
            .holdings
            .iter()
            .find(|h| (code.is_empty() && h.asset_code != "XRP") || h.asset_code == code)
    }?;
    let held = holding.amount.saturating_sub(holding.reserve);
    let micros = if holding.asset_scale >= 6 {
        held / 10_u64.pow(holding.asset_scale - 6)
    } else {
        held * 10_u64.pow(6 - holding.asset_scale)
    };
    Some(micros * 1_000)
}

/// The node's accounts in its hubs, as its owner sees them.
pub async fn hub_statuses(node: &TestNode) -> Vec<proto::HubStatus> {
    match node
        .ipc()
        .await
        .call(Method::GetHubStatus(proto::GetHubStatusRequest {}))
        .await
    {
        Ok(response::Method::GetHubStatus(reply)) => reply.hubs,
        other => panic!("{other:?}"),
    }
}

/// The node's account in the hub of `scheme`.
pub async fn hub_status(node: &TestNode, scheme: &str) -> proto::HubStatus {
    hub_statuses(node)
        .await
        .into_iter()
        .find(|status| status.scheme == scheme)
        .unwrap()
}

/// The node's settlement channels.
pub async fn channels(node: &TestNode) -> Vec<proto::SettlementChannel> {
    match node
        .ipc()
        .await
        .call(Method::ListSettlementChannels(
            proto::ListSettlementChannelsRequest {},
        ))
        .await
    {
        Ok(response::Method::ListSettlementChannels(list)) => list.channels,
        other => panic!("{other:?}"),
    }
}

/// The node's peers.
pub async fn peers(node: &TestNode) -> Vec<proto::NetworkPeer> {
    match node
        .ipc()
        .await
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await
    {
        Ok(response::Method::ListPeers(list)) => list.peers,
        other => panic!("{other:?}"),
    }
}

/// The node's ledger accounts.
pub async fn accounts(node: &TestNode) -> Vec<proto::Account> {
    match node
        .ipc()
        .await
        .call(Method::ListAccounts(proto::ListAccountsRequest {}))
        .await
    {
        Ok(response::Method::ListAccounts(list)) => list.accounts,
        other => panic!("{other:?}"),
    }
}

/// The ledgers on which `node` keeps a peer account for `peer`, sorted:
/// one for each method of their peering, a hub's ledger each.
pub async fn peer_ledgers(node: &TestNode, peer: &IdentityKey) -> Vec<String> {
    let suffix = format!(":assets/interledger/{}", peer.node_id());
    let mut ids: Vec<String> = accounts(node)
        .await
        .into_iter()
        .filter_map(|account| account.path.strip_suffix(&suffix).map(str::to_owned))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// What each of `members` owes each other net on ledger `id`, by its own
/// books, by `(member, peer)`: its peer account less the credit it extends,
/// negative when the peer owes it.
pub async fn peer_accounts(members: &[&Member], id: &str) -> BTreeMap<(u8, u8), i128> {
    let mut owed = BTreeMap::new();
    for member in members {
        for account in accounts(&member.node).await {
            for kind in ["assets/interledger", "contra/trust"] {
                let Some(peer) = account.path.strip_prefix(&format!("{id}:{kind}/")) else {
                    continue;
                };
                let Some(peer) = members.iter().find(|m| m.key.node_id().to_string() == peer)
                else {
                    continue;
                };
                let net = i128::from(account.credits_posted) - i128::from(account.debits_posted);
                *owed.entry((member.seed, peer.seed)).or_default() += net;
            }
        }
    }
    owed
}

/// A client of `from` with a BTP token on ledger `ledger` pays `amount`
/// over STREAM to a new incoming payment of `to`'s on ledger `receive`;
/// the incoming payment, once paid.
pub async fn pay(
    clock: &PausedClock,
    (from, to): (&TestNode, &TestNode),
    ledger: &str,
    amount: u64,
    receive: &str,
) -> proto::IncomingPayment {
    let token = match from
        .ipc()
        .await
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "client".into(),
            ledger: ledger.into(),
            cap: None,
        }))
        .await
    {
        Ok(response::Method::CreateBtpToken(token)) => token,
        other => panic!("no token: {other:?}"),
    };
    let mut owner = to.ipc().await;
    let _ = owner
        .call(Method::SetHostname(proto::SetHostnameRequest {
            hostname: "localhost".into(),
        }))
        .await;
    let created = owner
        .call(Method::CreateIncomingPayment(
            proto::CreateIncomingPaymentRequest {
                ledger: receive.into(),
                ..proto::CreateIncomingPaymentRequest::default()
            },
        ))
        .await;
    let Ok(response::Method::CreateIncomingPayment(created)) = created else {
        panic!("no incoming payment: {created:?}");
    };
    let receiver = created.payment.unwrap();
    let mut client = BtpClient::connect(from, &token.secret).await.unwrap();
    // While nothing is in flight, the daemons' time passes, once they are
    // quiescent, for the members to fund their holds over the hub.
    pay_stream(
        clock,
        &mut client,
        &receiver.destination_account,
        &receiver.shared_secret,
        amount,
    )
    .await;
    incoming(to, &receiver.id).await
}

/// The incoming payment `id` of `node`.
pub async fn incoming(node: &TestNode, id: &str) -> proto::IncomingPayment {
    match node
        .ipc()
        .await
        .call(Method::ListIncomingPayments(
            proto::ListIncomingPaymentsRequest { limit: 0 },
        ))
        .await
    {
        Ok(response::Method::ListIncomingPayments(list)) => {
            list.payments.into_iter().find(|p| p.id == id).unwrap()
        }
        other => panic!("{other:?}"),
    }
}
