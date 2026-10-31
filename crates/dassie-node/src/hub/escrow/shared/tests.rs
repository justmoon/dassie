//! A link watcher serving several escrows against the fake XRP Ledger:
//! one connection and one anchoring round for all of them, each escrow
//! attesting only its own, a reset attested once per hub and kept with
//! every cursor, a fair share of what is bounded for each, an escrow that
//! joins a live link catching up on it, and one escrow's reasons ending the
//! connection only every so often.

use dassie_hub::escrow::{escrow_master, setup};
use dassie_hub::ledger::TESTNET_NETWORK;
use dassie_identity::IdentityKey;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::{Amount, Payment, Signing, TokenValue, Transact, TrustSet};
use dassie_xrpl::fake::{
    CLOSE_INTERVAL_S, DSUSD_CURRENCY, DSUSD_ISSUER, FEE_BASE, FakeLedger, KeyedSignatures,
    LIST_SITE,
};
use dassie_xrpl::unl::ledger_seconds;
use dassie_xrpl::verifier::{ANCHOR_EVERY_MS, MAX_QUERIES};

use super::super::link::ESCROW_RECONNECT_MS;
use super::super::{KEEP_VERIFIER_EVERY_MS, RETRY_MS, Watcher};
use super::*;
use crate::hub::config::tests::{NONCE, rules};

/// The secret the fake signs its validations under.
const SECRET: [u8; 32] = *b"the link's tests' validations...";

/// The connection the scene is set up over: never the watcher's.
const SETUP: u64 = 1_000;

/// The most outputs one input may take to settle.
const MAX_TURNS: usize = 100_000;

/// DSUSD, a token hub's asset.
const DSUSD: IssuedAsset = IssuedAsset {
    issuer: DSUSD_ISSUER,
    currency: DSUSD_CURRENCY,
};

/// One hub whose escrow the link follows, and what its host made of it.
struct Hub {
    id: HubId,
    view: EscrowView,
    master: Wallet,
    attested: Vec<ValidatorOperation>,
    cursor: Option<EscrowCursor>,
}

impl Hub {
    fn escrow(&self) -> AccountId {
        AccountId(self.view.rules.terms.escrow)
    }

    /// The deposits it attested, by tag and amount.
    fn deposits(&self) -> Vec<(u32, u64)> {
        (self.attested.iter())
            .filter_map(|op| match op {
                ValidatorOperation::AttestDeposit(deposit) => Some((deposit.tag, deposit.amount)),
                _ => None,
            })
            .collect()
    }

    fn resets(&self) -> usize {
        (self.attested.iter())
            .filter(|op| matches!(op, ValidatorOperation::AttestReset { .. }))
            .count()
    }
}

struct Scene {
    xrpl: FakeLedger,
    watcher: LinkWatcher,
    hubs: Vec<Hub>,
    /// The watcher's connection.
    connection: u64,
    now: u64,
    /// How often the watcher asked for a new connection.
    reconnects: u32,
    /// What the watcher last handed over of its verifier.
    kept: Option<Vec<u8>>,
    /// The cursors each [`LinkOut::Keep`] carried, in order.
    keeps: Vec<Vec<(HubId, EscrowCursor)>>,
    problems: Vec<&'static str>,
    /// Every request the watcher sent, in order.
    sent: Vec<String>,
    /// How long an answer takes to come back: none, at once; else on the
    /// next round ([`Scene::rounds`]).
    round_trip_ms: u64,
    on_the_way: Vec<Vec<u8>>,
    /// The account that holds DSUSD and deposits it.
    holder: Wallet,
    /// The most queries the first hub's escrow had open at once.
    most_open: usize,
    /// An escrow whose history the server refuses to read, and how many
    /// more times.
    refusing: Option<(AccountId, u32)>,
}

/// The publishers of the fake ledger, as a validator's file names them.
fn fake_publishers() -> UnlConfig {
    let xrpl = FakeLedger::new();
    UnlConfig::for_network(TESTNET_NETWORK)
        .and_then(|own| own.with_override(&xrpl.publishers(LIST_SITE)))
        .unwrap()
}

fn link_watcher() -> LinkWatcher {
    LinkWatcher::new(
        TESTNET_NETWORK,
        fake_publishers(),
        Box::new(KeyedSignatures::new(SECRET)),
    )
}

impl Scene {
    /// Hubs of `assets`, XRP or a token, whose sponsors funded their
    /// escrows and have yet to set them up ([`Self::set_up`]); none
    /// registered yet.
    fn funded(assets: &[Option<IssuedAsset>]) -> Self {
        let mut xrpl = FakeLedger::new();
        xrpl.sign_validations_with(SECRET);
        let holder = Wallet::from_seed([2; 16]);
        xrpl.deposit(holder.account(), 100_000_000);
        let nodes: Vec<(IdentityKey, String)> = (1..=4)
            .map(|i| {
                (
                    dassie_identity::TableKeygen::identity_key(&[i; 32]),
                    format!("v{i}.example:443"),
                )
            })
            .collect();
        let mut hubs = Vec::new();
        for (index, asset) in (100_u8..).zip(assets) {
            let sponsor = dassie_identity::TableKeygen::identity_key(&[index; 32]);
            let mut rules = rules(&sponsor, &nodes);
            if let Some(token) = asset {
                rules.terms.asset = token.asset();
            }
            let master = escrow_master(&sponsor, TESTNET_NETWORK, &NONCE);
            xrpl.fund(master.account());
            let view = EscrowView {
                rules: rules.clone(),
                checked: true,
                next_sequence: 0,
                withdrawals: Vec::new(),
                signer_list: None,
                written_off_at: None,
                issuer: Issuer::default(),
                deposit_room: 4,
                deposits_recorded: Vec::new(),
            };
            hubs.push(Hub {
                id: rules.id(),
                view,
                master,
                attested: Vec::new(),
                cursor: None,
            });
        }
        xrpl.close();
        let mut scene = Self {
            now: xrpl.now_ms() + 1_000,
            xrpl,
            watcher: link_watcher(),
            hubs,
            connection: 1,
            reconnects: 0,
            kept: None,
            keeps: Vec::new(),
            problems: Vec::new(),
            sent: Vec::new(),
            round_trip_ms: 0,
            on_the_way: Vec::new(),
            holder,
            most_open: 0,
            refusing: None,
        };
        if assets.iter().any(Option::is_some) {
            scene.hold_dsusd();
        }
        scene
    }

    /// Hubs of `assets`, set up and registered on a link not yet connected.
    fn new(assets: &[Option<IssuedAsset>]) -> Self {
        let mut scene = Self::funded(assets);
        scene.set_up();
        for index in 0..scene.hubs.len() {
            scene.register(index);
        }
        scene
    }

    /// Each sponsor sets up its escrow, in a ledger that closes now.
    fn set_up(&mut self) {
        for hub in &mut self.hubs {
            let master = hub.master.account();
            let sequence = self.xrpl.sequence(master).unwrap();
            for transaction in setup(&hub.view.rules, &hub.master, sequence, FEE_BASE) {
                self.xrpl
                    .request(SETUP, &api::submit(2, transaction.blob()));
            }
        }
        self.xrpl.close();
        for hub in &mut self.hubs {
            hub.view.next_sequence = self.xrpl.sequence(hub.master.account()).unwrap();
        }
    }

    /// The link follows hub `index`'s escrow from now on, from its cursor.
    fn register(&mut self, index: usize) {
        let hub = &self.hubs[index];
        let (id, cursor, view) = (hub.id, hub.cursor, hub.view.clone());
        self.watcher.register(self.now, id, cursor, &view);
        self.run();
    }

    fn views(&self) -> BTreeMap<HubId, EscrowView> {
        (self.hubs.iter())
            .map(|hub| (hub.id, hub.view.clone()))
            .collect()
    }

    fn hub_mut(&mut self, id: HubId) -> &mut Hub {
        self.hubs.iter_mut().find(|hub| hub.id == id).unwrap()
    }

    /// The holder opens a line to DSUSD's issuer and is issued some.
    fn hold_dsusd(&mut self) {
        let holder = &self.holder;
        let trust = TrustSet {
            account: holder.account(),
            limit: DSUSD.amount(TokenValue::parse("1000").unwrap()),
            fee: FEE_BASE,
            sequence: self.xrpl.sequence(holder.account()).unwrap(),
            last_ledger_sequence: None,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(holder.public_key()),
        }
        .sign(holder);
        self.xrpl.request(SETUP, &api::submit(3, &trust.blob));
        self.xrpl.close();
        let held = DSUSD.amount(TokenValue::parse("50").unwrap());
        self.xrpl.issue(holder.account(), held).unwrap();
        self.xrpl.close();
    }
}

/// `from` pays `amount` to `to` with tag `tag`, in the next ledger.
fn pay(xrpl: &mut FakeLedger, from: &Wallet, to: AccountId, amount: Amount, tag: u32) {
    {
        let paid = Payment {
            account: from.account(),
            destination: to,
            destination_tag: Some(tag),
            amount,
            send_max: None,
            fee: FEE_BASE,
            sequence: xrpl.sequence(from.account()).unwrap(),
            last_ledger_sequence: None,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(from.public_key()),
        }
        .sign(from);
        let answer = xrpl.request(SETUP, &api::submit(4, &paid.blob));
        let Message::Response { outcome, .. } = api::parse(&answer) else {
            panic!("no answer");
        };
        assert_eq!(
            api::submit_result(&outcome.unwrap()).as_deref(),
            Some("tesSUCCESS")
        );
    }
}

impl Scene {
    /// Carries out what the watcher asks, as its host does, until it asks
    /// nothing more.
    fn run(&mut self) {
        for _ in 0..MAX_TURNS {
            self.watcher.check_invariants();
            if let Some(first) = self.hubs.first().map(|hub| hub.id) {
                let open = self
                    .watcher
                    .escrows
                    .get(&first)
                    .map_or(0, |e| e.open_queries());
                self.most_open = self.most_open.max(open);
            }
            let Some(out) = self.watcher.poll() else {
                return;
            };
            self.carry_out(out);
        }
        panic!("the watcher does not settle");
    }

    fn carry_out(&mut self, out: LinkOut) {
        match out {
            LinkOut::Send(bytes) => {
                self.sent.push(String::from_utf8_lossy(&bytes).into_owned());
                let answer = self
                    .refused(&bytes)
                    .unwrap_or_else(|| self.xrpl.request(self.connection, &bytes));
                if self.round_trip_ms == 0 {
                    let views = self.views();
                    self.watcher.message(self.now, &answer, &views);
                } else {
                    self.on_the_way.push(answer);
                }
            }
            LinkOut::Fetch(url) => {
                let body = self.xrpl.fetch(&url);
                self.watcher.fetched(self.now, &url, body.as_deref());
            }
            LinkOut::Reconnect => {
                self.reconnects += 1;
                self.reconnect();
            }
            LinkOut::Attest(id, operation, transaction) => {
                let deposit = matches!(operation, ValidatorOperation::AttestDeposit(_));
                let hub = self.hub_mut(id);
                hub.attested.push(operation);
                if let (true, Some(hash)) = (deposit, transaction) {
                    hub.view.deposits_recorded.push(hash);
                }
                self.watcher.taken(id, transaction);
            }
            LinkOut::Cursor(id, cursor) => self.hub_mut(id).cursor = Some(cursor),
            LinkOut::Keep(kept, cursors) => {
                self.kept = match kept {
                    Kept::State(state) => Some(state),
                    Kept::Dropped => None,
                };
                for (id, cursor) in &cursors {
                    self.hub_mut(*id).cursor = Some(*cursor);
                }
                self.keeps.push(cursors);
            }
            LinkOut::Problem(problem) | LinkOut::HubProblem(_, problem) => {
                self.problems.push(problem);
            }
        }
    }

    /// The server's refusal of `request`, if it reads the history of the
    /// escrow it refuses.
    fn refused(&mut self, request: &[u8]) -> Option<Vec<u8>> {
        let (escrow, times) = self.refusing.as_mut()?;
        let request: serde_json::Value = serde_json::from_slice(request).ok()?;
        let reads = request.get("command")? == "account_tx"
            && request.get("account")? == escrow.to_address().as_str();
        if !reads || *times == 0 {
            return None;
        }
        *times -= 1;
        let refusal = serde_json::json!({
            "type": "response",
            "id": request.get("id")?,
            "status": "error",
            "error": "tooBusy",
        });
        serde_json::to_vec(&refusal).ok()
    }

    /// A new connection.
    fn reconnect(&mut self) {
        self.on_the_way.clear();
        self.watcher.disconnected();
        self.xrpl.disconnect(self.connection);
        self.connection += 1;
        self.watcher.connected(self.now);
    }

    fn connect(&mut self) {
        self.watcher.connected(self.now);
        self.run();
        self.tick(0);
    }

    /// A ledger closes, [`CLOSE_INTERVAL_S`] on, and what the streams
    /// carry is passed on; over a slow connection, the time passes in
    /// rounds first.
    fn close(&mut self) {
        match (CLOSE_INTERVAL_S * 1_000).checked_div(self.round_trip_ms) {
            None => self.now += CLOSE_INTERVAL_S * 1_000,
            Some(rounds) => {
                for _ in 0..rounds {
                    self.now += self.round_trip_ms;
                    let views = self.views();
                    for answer in std::mem::take(&mut self.on_the_way) {
                        self.watcher.message(self.now, &answer, &views);
                    }
                    self.run();
                    self.tick(0);
                }
            }
        }
        let streamed = self.xrpl.close_at(ledger_seconds(self.now));
        let views = self.views();
        for (connection, bytes) in streamed {
            if connection == self.connection {
                self.watcher.message(self.now, &bytes, &views);
            }
        }
        self.run();
        self.tick(0);
    }

    fn closes(&mut self, ledgers: u32) {
        for _ in 0..ledgers {
            self.close();
        }
    }

    /// `ms` pass.
    fn tick(&mut self, ms: u64) {
        self.now += ms;
        let views = self.views();
        self.watcher.tick(self.now, &views);
        self.run();
    }

    /// How many requests of `command` it sent.
    fn asked(&self, command: &str) -> usize {
        let named = format!("\"command\":\"{command}\"");
        self.sent
            .iter()
            .filter(|sent| sent.contains(&named))
            .count()
    }
}

// One connection, one check of where the server stands and one anchoring
// round per closed ledger serve both escrows: the second costs its own
// subscription and history, and no anchoring of its own.
#[test]
fn two_escrows_share_one_connection_and_one_anchoring_round() {
    let mut one = Scene::new(&[None]);
    let mut two = Scene::new(&[None, None]);
    for scene in [&mut one, &mut two] {
        scene.connect();
        scene.closes(5);
    }
    for hub in &two.hubs {
        assert!(two.watcher.following(hub.id));
        assert!(two.watcher.fresh(hub.id), "each took the anchors");
    }
    assert_eq!(two.reconnects, 0, "{:?}", two.problems);
    assert_eq!(two.asked("server_state"), 1);
    assert_eq!(two.asked("subscribe"), 2);
    assert_eq!(
        two.asked("validations"),
        one.asked("validations"),
        "the anchoring rounds of one escrow"
    );
    assert!(two.asked("validations") > 0);
    let anchors: Vec<_> = (two.hubs.iter())
        .map(|hub| two.watcher.anchor(hub.id))
        .collect();
    assert!(
        anchors.windows(2).all(|pair| pair[0] == pair[1]),
        "the same anchor"
    );
}

// Deposits to each escrow, and a payment from one escrow to the other,
// are attested by the hub whose deposit each is, and by no other: the
// verifier threads both escrows' objects, and each keeps its own threads.
#[test]
fn each_escrow_attests_only_its_own() {
    let mut scene = Scene::funded(&[None, None]);
    let (a, b) = (scene.hubs[0].escrow(), scene.hubs[1].escrow());
    pay(
        &mut scene.xrpl,
        &scene.hubs[0].master,
        b,
        Amount::Xrp(3_000_000),
        9,
    );
    scene.xrpl.close();
    scene.set_up();
    for index in 0..2 {
        scene.register(index);
    }
    let union: BTreeSet<[u8; 32]> = [a, b].map(dassie_xrpl::records::account_key).into();
    assert_eq!(
        scene.watcher.link.threads(),
        union.into_iter().collect::<Vec<_>>()
    );
    scene.connect();
    scene.xrpl.deposit_tagged(a, 5_000_000, Some(7));
    scene.xrpl.deposit_tagged(b, 6_000_000, Some(8));
    scene.closes(4);
    assert_eq!(scene.hubs[0].deposits(), [(7, 5_000_000)]);
    assert_eq!(scene.hubs[1].deposits(), [(9, 3_000_000), (8, 6_000_000)]);
    assert_eq!(scene.reconnects, 0, "{:?}", scene.problems);
}

/// A streamed transaction's hash, and the message.
type Streamed = ([u8; 32], Vec<u8>);

/// What the stream carries as the next ledger closes, on the watcher's
/// connection: its transactions, each with its hash, and the rest.
fn close_streamed(scene: &mut Scene) -> (Vec<Streamed>, Vec<Vec<u8>>) {
    scene.now += CLOSE_INTERVAL_S * 1_000;
    let streamed = scene.xrpl.close_at(ledger_seconds(scene.now));
    let mut transactions = Vec::new();
    let mut rest = Vec::new();
    for (connection, bytes) in streamed {
        if connection != scene.connection {
            continue;
        }
        match api::parse(&bytes) {
            Message::Transaction(record) => transactions.push((record.hash, bytes)),
            _ => rest.push(bytes),
        }
    }
    (transactions, rest)
}

/// The hubs whose escrows hold a hint at `hash`, by index.
fn hinted(scene: &Scene, hash: &[u8; 32]) -> Vec<usize> {
    (0..scene.hubs.len())
        .filter(|index| scene.watcher.escrows[&scene.hubs[*index].id].hinted_at(hash))
        .collect()
}

// A streamed transaction hints at itself to the escrows it names: a
// deposit to the escrow it pays, a token hub's issuer's clawback, which
// names neither escrow, to the escrow of the issuer's token; one that
// names no escrow, to every escrow. Each attests only its own: the token
// hub's escrow the clawback, and the XRP hub's escrow nothing of it.
#[test]
fn a_clawback_reaches_the_token_escrow() {
    let mut scene = Scene::new(&[None, Some(DSUSD)]);
    scene.round_trip_ms = 500;
    scene.connect();
    scene.closes(2);
    let (xrp, token) = (scene.hubs[0].escrow(), scene.hubs[1].escrow());
    let five = DSUSD.amount(TokenValue::parse("5").unwrap());
    pay(
        &mut scene.xrpl,
        &scene.holder,
        token,
        Amount::Token(five),
        7,
    );
    scene.xrpl.deposit_tagged(xrp, 2_000_000, Some(9));
    let (transactions, _) = close_streamed(&mut scene);
    assert_eq!(transactions.len(), 2);
    let views = scene.views();
    let mut paid = Vec::new();
    for (hash, bytes) in &transactions {
        scene.watcher.message(scene.now, bytes, &views);
        let to = hinted(&scene, hash);
        assert!(to == [0] || to == [1], "{to:?}");
        paid.push(to[0]);
    }
    assert_eq!(paid.iter().collect::<BTreeSet<_>>().len(), 2, "one each");

    // Paid to no escrow, the same hints every escrow. What its server
    // hinted at goes with the connection.
    hint_elsewhere(&mut scene, &transactions, &paid, &views);
    scene.reconnect();
    scene.run();
    scene.closes(3);
    assert_eq!(scene.hubs[1].deposits(), [(7, 5_000_000)]);
    assert_eq!(scene.hubs[0].deposits(), [(9, 2_000_000)]);

    let clawed = DSUSD.amount(TokenValue::parse("1.5").unwrap());
    scene.xrpl.claw_back(token, clawed).unwrap();
    let (transactions, rest) = close_streamed(&mut scene);
    let [(hash, bytes)] = transactions.as_slice() else {
        panic!("{transactions:?}");
    };
    let views = scene.views();
    scene.watcher.message(scene.now, bytes, &views);
    assert_eq!(hinted(&scene, hash), [1], "to the token's escrow");
    for bytes in rest {
        scene.watcher.message(scene.now, &bytes, &views);
    }
    scene.run();
    scene.closes(3);
    let clawbacks: Vec<u64> = (scene.hubs[1].attested.iter())
        .filter_map(|op| match op {
            ValidatorOperation::AttestClawback(clawback) => Some(clawback.amount),
            _ => None,
        })
        .collect();
    assert_eq!(clawbacks, [1_500_000]);
    let attested = &scene.hubs[0].attested;
    assert!(
        !attested
            .iter()
            .any(|op| matches!(op, ValidatorOperation::AttestClawback(_))),
        "{attested:?}"
    );
}

/// Each of `transactions`, paid to the escrow of hub `paid[index]`, made
/// up to pay no escrow, hints every escrow.
fn hint_elsewhere(
    scene: &mut Scene,
    transactions: &[Streamed],
    paid: &[usize],
    views: &BTreeMap<HubId, EscrowView>,
) {
    let elsewhere = Wallet::from_seed([9; 16]).account().to_address();
    for (index, ((hash, bytes), to)) in transactions.iter().zip(paid).enumerate() {
        let made_up = [u8::try_from(index).unwrap(); 32];
        let text = String::from_utf8_lossy(bytes)
            .replace(&scene.hubs[*to].escrow().to_address(), &elsewhere)
            .replace(&hex_upper(hash), &hex_upper(&made_up));
        scene.watcher.message(scene.now, text.as_bytes(), views);
        assert_eq!(hinted(scene, &made_up), [0, 1], "to every escrow");
    }
}

/// `bytes` in upper-case hex, as the server names hashes.
fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

// spec/decisions/ledger-resets.md: a reset of the test network is attested
// once by each hub, the server is asked where it stands as often as for
// one escrow, and the new chain's anchor is kept at once with every
// escrow's cursor of the new chain, in one write: a link restarted from it
// attests the reset no second time.
#[test]
fn a_reset_is_attested_once_per_hub_and_kept_with_every_cursor() {
    let mut one = Scene::new(&[None]);
    let mut two = Scene::new(&[None, None]);
    let mut asked = Vec::new();
    for scene in [&mut one, &mut two] {
        scene.connect();
        scene.closes(2);
        scene.tick(KEEP_VERIFIER_EVERY_MS);
        let before = scene.asked("server_state");
        scene.xrpl.reset();
        scene.close();
        scene.watcher.disconnected();
        scene.connect();
        scene.closes(2);
        asked.push(scene.asked("server_state") - before);
    }
    assert_eq!(asked[0], asked[1], "one check of the server per reset");
    let old = two.keeps.first().unwrap()[0].1;
    assert!(two.hubs.iter().all(|hub| hub.resets() == 1));
    let kept = two.keeps.last().unwrap();
    assert_eq!(kept.len(), 2, "with every cursor");
    assert!(
        kept.iter()
            .all(|(_, cursor)| cursor.newest_ledger < old.newest_ledger),
        "the new chain's: {kept:?}"
    );

    let mut restarted = link_watcher();
    let state = two.kept.clone().unwrap();
    let signatures = Box::new(KeyedSignatures::new(SECRET));
    assert_eq!(restarted.restore(&state, signatures, two.now), Ok(()));
    two.watcher = restarted;
    for hub in &mut two.hubs {
        hub.attested.clear();
    }
    for index in 0..2 {
        two.register(index);
    }
    two.connect();
    two.closes(2);
    assert!(two.hubs.iter().all(|hub| hub.resets() == 0));
    assert!(two.hubs.iter().all(|hub| two.watcher.following(hub.id)));

    // Each escrow that meets the same reset asks where the server stands
    // again: the link asks once; a later reset asks again.
    let sends = |scene: &mut Scene| {
        std::iter::from_fn(|| scene.watcher.poll())
            .filter(|out| matches!(out, LinkOut::Send(_)))
            .count()
    };
    let anchor = two.watcher.anchor(two.hubs[0].id).unwrap().index;
    for id in [two.hubs[0].id, two.hubs[1].id] {
        two.watcher.with(id, |_, link| link.check_again(anchor));
    }
    assert_eq!(sends(&mut two), 1);
    two.watcher
        .with(two.hubs[0].id, |_, link| link.check_again(anchor + 1));
    assert_eq!(sends(&mut two), 1);
}

// However many hints one escrow's stream brings, it proves them within its
// share of the verifier's queries, and the escrow beside it has its own
// left: a deposit to it is attested within the ledgers it takes alone.
#[test]
fn a_flood_of_hints_to_one_escrow_leaves_the_other_its_share() {
    let mut scene = Scene::new(&[None, None, None, None]);
    scene.round_trip_ms = 500;
    scene.connect();
    scene.closes(2);
    let (a, b) = (scene.hubs[0].escrow(), scene.hubs[1].escrow());
    let flood = 2 * api::PAGE;
    for tag in 0..flood {
        scene.xrpl.deposit_tagged(a, 1_000, Some(tag + 100));
    }
    scene.xrpl.deposit_tagged(b, 5_000_000, Some(7));
    scene.most_open = 0;
    scene.closes(2);
    assert_eq!(scene.hubs[1].deposits(), [(7, 5_000_000)], "in its share");
    let flooded = scene.hubs[0].deposits().len();
    assert!(
        flooded < usize::try_from(flood).unwrap(),
        "the flood takes longer"
    );
    assert!(scene.most_open <= MAX_QUERIES / 4, "{}", scene.most_open);
    assert!(scene.most_open > 0);
    for _ in 0..40 {
        scene.closes(1);
    }
    assert_eq!(
        scene.hubs[0].deposits().len(),
        usize::try_from(flood).unwrap()
    );
}

// An escrow registered on a link that follows another already subscribes
// and reads its history on the same connection: no new connection, no new
// check of the server, and the deposit made before it joined is attested.
#[test]
fn an_escrow_registered_on_a_live_link_catches_up_without_reconnecting() {
    let mut scene = Scene::funded(&[None, None]);
    scene.set_up();
    scene.register(0);
    scene.connect();
    scene.closes(2);
    let b = scene.hubs[1].escrow();
    scene.xrpl.deposit_tagged(b, 5_000_000, Some(7));
    scene.closes(2);
    scene.register(1);
    assert!(scene.watcher.following(scene.hubs[1].id), "at once");
    scene.closes(2);
    assert_eq!(scene.hubs[1].deposits(), [(7, 5_000_000)]);
    assert_eq!(scene.reconnects, 0, "{:?}", scene.problems);
    assert_eq!(scene.asked("server_state"), 1);
    assert!(scene.watcher.following(scene.hubs[0].id));
}

// A server that refuses one escrow's history leaves that escrow alone to
// subscribe again later: the link stays checked, and the other escrow
// follows on, on the same connection, attesting meanwhile.
#[test]
fn one_escrows_failed_history_leaves_the_others_following() {
    let mut scene = Scene::new(&[None, None]);
    let (a, b) = (scene.hubs[0].escrow(), scene.hubs[1].escrow());
    scene.refusing = Some((a, 1));
    scene.connect();
    let (first, second) = (scene.hubs[0].id, scene.hubs[1].id);
    assert!(!scene.watcher.following(first), "refused");
    assert!(scene.watcher.following(second));
    assert!(scene.watcher.link.checked(), "the link waits for no one");
    scene.xrpl.deposit_tagged(b, 5_000_000, Some(7));
    scene.closes(1);
    assert_eq!(scene.hubs[1].deposits(), [(7, 5_000_000)]);
    assert!(!scene.watcher.following(first), "not before its time");

    scene.xrpl.deposit_tagged(a, 6_000_000, Some(8));
    scene.closes(3);
    assert!(scene.watcher.following(first), "subscribed again");
    assert_eq!(scene.hubs[0].deposits(), [(8, 6_000_000)]);
    assert_eq!(scene.reconnects, 0, "{:?}", scene.problems);
    assert_eq!(scene.asked("server_state"), 1);
    const { assert!(RETRY_MS < 3 * CLOSE_INTERVAL_S * 1_000) };
}

// A new connection one escrow asks for goes at most every
// ESCROW_RECONNECT_MS, whichever escrow asks; one the verifier asks for
// goes at once, every time.
#[test]
fn escrow_reconnects_are_rate_limited() {
    let mut scene = Scene::new(&[None, None]);
    scene.connect();
    let ids: Vec<HubId> = scene.hubs.iter().map(|hub| hub.id).collect();
    let reconnects = |scene: &mut Scene| {
        let mut count = 0;
        while let Some(out) = scene.watcher.poll() {
            count += usize::from(out == LinkOut::Reconnect);
        }
        count
    };
    for id in &ids {
        scene.watcher.with(*id, |_, link| link.push(Out::Reconnect));
    }
    assert_eq!(reconnects(&mut scene), 1, "one for both escrows");
    scene.watcher.link.now(scene.now + ESCROW_RECONNECT_MS - 1);
    scene
        .watcher
        .with(ids[1], |_, link| link.push(Out::Reconnect));
    assert_eq!(reconnects(&mut scene), 0, "not before its time");
    scene.watcher.link.now(scene.now + ESCROW_RECONNECT_MS);
    scene
        .watcher
        .with(ids[1], |_, link| link.push(Out::Reconnect));
    assert_eq!(reconnects(&mut scene), 1, "then again");
    for _ in 0..2 {
        scene.watcher.link.carry_out(verifier::Out::Reconnect);
    }
    assert_eq!(reconnects(&mut scene), 2, "the verifier's at once");
    const { assert!(ESCROW_RECONNECT_MS > 2 * ANCHOR_EVERY_MS) };

    // A watcher of one escrow ends its connection whenever it asks.
    let mut alone = Watcher::new(
        scene.hubs[0].escrow(),
        TESTNET_NETWORK,
        None,
        fake_publishers(),
        Box::new(KeyedSignatures::new(SECRET)),
    );
    for _ in 0..2 {
        alone.link.push(Out::Reconnect);
    }
    let alone_reconnects = std::iter::from_fn(|| alone.poll())
        .filter(|out| *out == Out::Reconnect)
        .count();
    assert_eq!(alone_reconnects, 2);
}

// The bounds: at most MAX_VALIDATED_HUBS escrows, whose shares of the
// verifier's queries and of the link's requests fit the link's together;
// one unregistered lets go of its queries and its share.
#[test]
fn shares_fit_the_links_bounds_and_follow_the_escrows_served() {
    let mut scene = Scene::new(&[None, None, None, None]);
    let link = &scene.watcher.link;
    assert_eq!(link.query_share() * MAX_VALIDATED_HUBS, MAX_QUERIES);
    assert!(link.hint_share() * 2 <= link.query_share());
    scene.connect();
    scene.closes(2);
    let gone = scene.hubs[3].id;
    scene.watcher.unregister(gone);
    scene.hubs.pop();
    scene.watcher.check_invariants();
    assert_eq!(scene.watcher.link.query_share(), MAX_QUERIES / 3);
    assert_eq!(scene.watcher.hubs().count(), 3);
    scene.closes(2);
    assert!(scene.hubs.iter().all(|hub| scene.watcher.following(hub.id)));
}

#[test]
#[should_panic(expected = "escrows are bounded")]
fn no_more_escrows_than_a_node_validates_hubs() {
    let mut scene = Scene::funded(&[None, None, None, None, None]);
    scene.set_up();
    for index in 0..5 {
        scene.register(index);
    }
}
