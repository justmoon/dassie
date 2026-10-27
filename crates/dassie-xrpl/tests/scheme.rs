//! The `xrpl-testnet-xrp` scheme against the fake ledger: each rule of how it
//! books transactions and settles (spec/areas/xrpl.md#following-the-account,
//! spec/areas/xrpl.md#settling), restarts and catch-up, with the books
//! checked against the ledger after every step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};

use dassie_kit::sim::SeededRng;
use dassie_ledger::Name;
use dassie_settlement::{
    EntryPayment, ExternalAsset, LedgerInput, LedgerStatus, ReconcileCause, SchemeError,
};
use dassie_xrpl::account::{Doubt, HISTORY_BOUND, STALL_MS};
use dassie_xrpl::entry::ENTRY_HOLD_LEDGERS;
use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger, Lie, RESERVE_BASE};
use dassie_xrpl::scheme::MEMO_TYPE;
use support::{Effect, XrplScheme};

mod support;
use dassie_xrpl::{AccountId, UNITS_PER_DROP};

/// What the node would book for a scheme, in internal units.
#[derive(Debug, Default)]
struct Books {
    /// `assets/settlement`, as a debit balance.
    settlement: i128,
    owner: i128,
    suspense: i128,
    fees: i128,
    /// Each peer's account, as what we owe it net of what it settled.
    peers: BTreeMap<Name, i128>,
    /// Our settlements prepared: peer and amount.
    pending: BTreeMap<Name, (Name, u64)>,
    /// Settlements in, by ID: each once.
    credited: BTreeSet<Name>,
    finalized: Vec<Name>,
    cancelled: Vec<Name>,
    /// Each reconciliation, in order: what the books gained, and why.
    reconciled: Vec<(i128, ReconcileCause)>,
    /// The transaction of each fee, in order.
    fee_ids: Vec<Name>,
    /// The owner's payments held while on their way, as the node holds
    /// them: by ID, what each pays.
    held: BTreeMap<Name, u64>,
}

struct Node {
    name: Name,
    scheme: XrplScheme,
    connection: u64,
    online: bool,
    books: Books,
    /// Submissions the network loses.
    drop_submits: bool,
    /// Requests of this command go unanswered, and are counted.
    hold: Option<&'static str>,
    held: usize,
    /// The last request held.
    held_request: Option<serde_json::Value>,
    /// The transactions it asked the proofs of, in order.
    proved: Vec<String>,
    /// Whether its books may lag the ledger, as while its server lies.
    lagging: bool,
    /// A server behind the others, stopped where the ledger once was: the
    /// node's requests go to it while it is set.
    behind: Option<FakeLedger>,
    reconnects: usize,
    /// The commands it sent the ledger, in order.
    asked: Vec<String>,
}

struct World {
    ledger: FakeLedger,
    nodes: Vec<Node>,
    now: u64,
    next_connection: u64,
}

fn entropy(seed: u8) -> [u8; 32] {
    [seed; 32]
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

impl World {
    fn new(count: u8) -> Self {
        Self::on(FakeLedger::new(), count)
    }

    /// `count` nodes, connected to `ledger`.
    fn on(ledger: FakeLedger, count: u8) -> Self {
        // The node's clock reads the ledger's validations as fresh.
        let now = ledger.now_ms();
        let mut world = Self {
            ledger,
            nodes: Vec::new(),
            now,
            next_connection: 1,
        };
        for seed in 0..count {
            let node = name(&format!("node{seed}"));
            let scheme = XrplScheme::new("", &entropy(seed + 1), node, None, &[]).unwrap();
            world.nodes.push(Node {
                name: node,
                scheme,
                connection: 0,
                online: false,
                books: Books::default(),
                drop_submits: false,
                hold: None,
                held: 0,
                held_request: None,
                proved: Vec::new(),
                lagging: false,
                behind: None,
                reconnects: 0,
                asked: Vec::new(),
            });
        }
        for index in 0..world.nodes.len() {
            world.connect(index);
        }
        world
    }

    fn account(&self, index: usize) -> AccountId {
        self.nodes[index].scheme.account()
    }

    fn connect(&mut self, index: usize) {
        self.next_connection += 1;
        self.nodes[index].connection = self.next_connection;
        self.nodes[index].online = true;
        self.input(index, LedgerInput::Connected);
    }

    fn disconnect(&mut self, index: usize) {
        let connection = self.nodes[index].connection;
        self.ledger.disconnect(connection);
        self.nodes[index].online = false;
        self.input(index, LedgerInput::Disconnected);
    }

    fn input(&mut self, index: usize, input: LedgerInput<'_>) {
        let mut effects = Vec::new();
        self.nodes[index]
            .scheme
            .on_ledger(self.now, input, &mut effects);
        self.nodes[index].scheme.check_invariants();
        self.apply(index, effects);
        self.nodes[index].scheme.check_invariants();
    }

    /// Sends a request to the ledger, and its answer back, unless the
    /// node is offline or the network loses it.
    fn request(&mut self, index: usize, message: &[u8]) {
        let node = &mut self.nodes[index];
        if !node.online {
            return;
        }
        let request: serde_json::Value = serde_json::from_slice(message).unwrap();
        let command = request["command"].as_str().unwrap_or_default();
        node.asked.push(command.to_string());
        if let Some(hash) = request["tx_hash"].as_str() {
            node.proved.push(hash.to_owned());
        }
        // A transaction's bytes are read in binary, only to build the leaf
        // of a proof asked for (spec/designs/xrpl-validation.md#4-what-is-proven-and-how).
        if command == "tx" {
            assert_eq!(request["binary"], true, "{request}");
            let hash = request["transaction"].as_str().unwrap();
            assert!(node.proved.iter().any(|proved| proved == hash), "{request}");
        }
        if command == "submit" && node.drop_submits {
            return;
        }
        if node.hold == Some(command) {
            node.held += 1;
            node.held_request = Some(request);
            return;
        }
        let connection = node.connection;
        let reply = match &mut node.behind {
            Some(behind) => behind.request(connection, message),
            None => self.ledger.request(connection, message),
        };
        self.input(index, LedgerInput::Message(&reply));
    }

    /// Books the effects as the node would, and carries out the rest.
    fn apply(&mut self, index: usize, effects: Vec<Effect>) {
        for effect in effects {
            let node = &mut self.nodes[index];
            let books = &mut node.books;
            match effect {
                Effect::Deposit { amount } => {
                    books.settlement += i128::from(amount);
                    books.owner += i128::from(amount);
                }
                Effect::Withdrawal { amount } => {
                    books.settlement -= i128::from(amount);
                    books.owner -= i128::from(amount);
                }
                Effect::OwnerPaid { id, applied } => {
                    let amount = books.held.remove(&id).unwrap();
                    if applied {
                        books.settlement -= i128::from(amount);
                        books.owner -= i128::from(amount);
                    }
                }
                Effect::Reconcile {
                    amount,
                    gain,
                    cause,
                } => {
                    let amount = if gain {
                        i128::from(amount)
                    } else {
                        -i128::from(amount)
                    };
                    books.settlement += amount;
                    books.suspense += amount;
                    books.reconciled.push((amount, cause));
                }
                Effect::Fee { amount, id } => {
                    books.settlement -= i128::from(amount);
                    books.fees += i128::from(amount);
                    books.fee_ids.push(id);
                }
                Effect::IncomingSettlement { peer, amount, id } => {
                    assert!(books.credited.insert(id.unwrap()), "credited twice");
                    books.settlement += i128::from(amount);
                    *books.peers.entry(peer).or_default() += i128::from(amount);
                }
                Effect::Finalize { settlement } => {
                    let (peer, amount) = books.pending.remove(&settlement).unwrap();
                    books.settlement -= i128::from(amount);
                    *books.peers.entry(peer).or_default() -= i128::from(amount);
                    books.finalized.push(settlement);
                }
                Effect::Cancel { settlement } => {
                    books.pending.remove(&settlement).unwrap();
                    books.cancelled.push(settlement);
                }
                Effect::SendMessage { .. } => {}
                Effect::LedgerSend { message } => self.request(index, &message),
                Effect::Reconnect => {
                    node.reconnects += 1;
                    self.disconnect(index);
                    self.connect(index);
                }
                Effect::Fund { url, body } => {
                    assert!(url.starts_with("https://"));
                    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let to = AccountId::parse(request["destination"].as_str().unwrap()).unwrap();
                    self.ledger.fund(to);
                    self.input(index, LedgerInput::Funded(true));
                }
                Effect::Fetch { url } => self.fetch(index, &url),
            }
        }
    }

    /// Node `index` fetches `url`: the fake's validator list, from its
    /// site.
    fn fetch(&mut self, index: usize, url: &str) {
        let body = self.ledger.fetch(url);
        let fetched = LedgerInput::Fetched {
            url,
            body: body.as_deref(),
        };
        self.input(index, fetched);
    }

    /// Closes a ledger, and delivers the streams.
    fn close(&mut self) {
        self.now += 4_000;
        let close_time = dassie_xrpl::unl::ledger_seconds(self.now);
        for (connection, message) in self.ledger.close_at(close_time) {
            if let Some(index) = self
                .nodes
                .iter()
                .position(|n| n.online && n.connection == connection)
            {
                self.input(index, LedgerInput::Message(&message));
            }
        }
        self.timers();
        self.check();
    }

    /// Runs the nodes' timers that are due.
    fn timers(&mut self) {
        for index in 0..self.nodes.len() {
            if self.nodes[index]
                .scheme
                .deadline()
                .is_some_and(|at| at <= self.now)
            {
                let mut effects = Vec::new();
                self.nodes[index].scheme.on_timer(self.now, &mut effects);
                self.apply(index, effects);
            }
        }
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
        }
    }

    /// Time passes, by `ms`, without a ledger closing.
    fn wait(&mut self, ms: u64) {
        self.now += ms;
        self.timers();
        self.check();
    }

    /// `count` ledgers close while no node reads them, as fast as the fake
    /// closes them.
    fn pass(&mut self, count: u32) {
        for _ in 0..count {
            self.now += 4_000;
            self.ledger
                .close_at(dassie_xrpl::unl::ledger_seconds(self.now));
        }
    }

    /// The books of every synced node match its account: its balance less
    /// the reserve. And the ledger made and lost no XRP.
    fn check(&self) {
        for node in &self.nodes {
            assert_eq!(
                node.books.settlement,
                i128::from(node.scheme.balance()),
                "{}",
                node.name
            );
            // A reset the scheme has not seen yet is not its to know, nor
            // what its lying server keeps from it.
            if node.scheme.status() == LedgerStatus::Synced
                && !node.lagging
                && let Some(balance) = self.ledger.balance(node.scheme.account())
            {
                // Less what it holds of entry payments, which is off the
                // books (spec/decisions/pay-to-enter.md).
                let bookable = (balance * UNITS_PER_DROP)
                    .saturating_sub(node.scheme.reserve())
                    .saturating_sub(node.scheme.entries_held());
                assert_eq!(node.scheme.balance(), bookable, "{}", node.name);
            }
        }
        assert_eq!(self.ledger.total(), dassie_xrpl::MAX_DROPS);
    }

    /// Peers `a` and `b`, as the peer protocol would: `a` asks.
    fn peer(&mut self, a: usize, b: usize) -> (Vec<u8>, Vec<u8>) {
        let (name_a, name_b) = (self.nodes[a].name, self.nodes[b].name);
        let mut info = Vec::new();
        self.nodes[b].scheme.peering_info(&mut info);
        let mut request = Vec::new();
        self.nodes[a]
            .scheme
            .create_peering_request(name_b, &info, &mut request)
            .unwrap();
        let mut response = Vec::new();
        let state_at_b = self.nodes[b]
            .scheme
            .accept_peering_request(name_a, &request, &mut response)
            .unwrap();
        let state_at_a = self.nodes[a]
            .scheme
            .finalize_peering_request(name_b, &info, &response)
            .unwrap();
        self.nodes[b].scheme.on_peered(name_a, &state_at_b);
        self.nodes[a].scheme.on_peered(name_b, &state_at_a);
        (state_at_a, state_at_b)
    }

    /// Node `index`'s owner pays `payment` out of the account, which the
    /// node holds against the owner's funds while it is on its way.
    fn pay_owner(
        &mut self,
        index: usize,
        payment: &dassie_settlement::OwnerPayment,
        effects: &mut Vec<Effect>,
    ) -> Result<Name, SchemeError> {
        let id = self.nodes[index].scheme.pay_owner(payment, effects)?;
        self.nodes[index]
            .books
            .held
            .insert(id, payment.amount / UNITS_PER_DROP * UNITS_PER_DROP);
        Ok(id)
    }

    /// `from` settles `amount` with `to`, whose state at `from` is
    /// `state`. Returns the settlement's ID.
    fn settle(&mut self, from: usize, to: usize, amount: u64, state: &[u8]) -> Name {
        let peer = self.nodes[to].name;
        let mut rng = SeededRng::new(1);
        let prepared = self.nodes[from]
            .scheme
            .prepare_settlement(&mut rng, peer, amount, state)
            .unwrap();
        self.nodes[from]
            .books
            .pending
            .insert(prepared.id, (peer, prepared.amount));
        let mut effects = Vec::new();
        self.nodes[from]
            .scheme
            .execute_settlement(prepared.id, &mut effects);
        self.apply(from, effects);
        prepared.id
    }
}

/// Two funded, synced nodes, peered: the states each keeps for the other.
fn funded_pair() -> (World, Vec<u8>, Vec<u8>) {
    let mut world = World::new(2);
    world.closes(2);
    for node in &world.nodes {
        assert_eq!(node.scheme.status(), LedgerStatus::Synced, "{}", node.name);
    }
    let (state_a, state_b) = world.peer(0, 1);
    (world, state_a, state_b)
}

const FUNDED: u64 = (FAUCET_DROPS - RESERVE_BASE) * UNITS_PER_DROP;

// An account the faucet creates is a deposit, the reserve
// left out.
#[test]
fn a_new_account_is_funded_and_its_reserve_never_booked() {
    let (world, _, _) = funded_pair();
    for node in &world.nodes {
        assert_eq!(node.books.owner, i128::from(FUNDED));
        assert_eq!(node.scheme.reserve(), RESERVE_BASE * UNITS_PER_DROP);
        assert!(node.scheme.deposit_address().unwrap().starts_with('r'));
    }
}

// A settlement is whole drops; it is posted once it
// succeeded, its fee booked, and the peer credits what was delivered,
// once.
#[test]
fn a_settlement_settles_both_books() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 5_000_000_999, &state_a);
    world.close();
    let (a, b) = (&world.nodes[0].books, &world.nodes[1].books);
    assert_eq!(a.finalized, [id]);
    assert_eq!(a.peers[&world.nodes[1].name], -5_000_000_000);
    assert_eq!(a.fees, 10 * i128::from(UNITS_PER_DROP));
    assert_eq!(b.peers[&world.nodes[0].name], 5_000_000_000);
    assert_eq!(b.credited.len(), 1);
    assert_eq!(b.owner, i128::from(FUNDED));
}

// A Payment with our memo from an address that is not the peer's is a
// deposit: attribution cannot be spoofed.
#[test]
fn a_settlement_from_the_wrong_address_is_a_deposit() {
    let mut world = World::new(3);
    world.closes(2);
    // Node 0 knows node 1's address; node 2 sends claiming to be node 1.
    let (_, _) = world.peer(0, 1);
    let mut info = Vec::new();
    world.nodes[0].scheme.peering_info(&mut info);
    let impostor = world.nodes[2].name;
    world.nodes[2].name = world.nodes[1].name;
    let mut request = Vec::new();
    world.nodes[2]
        .scheme
        .create_peering_request(name("node0"), &info, &mut request)
        .unwrap();
    let _ = impostor;
    world.settle(2, 0, 1_000_000_000, &info);
    world.close();
    let books = &world.nodes[0].books;
    assert!(books.credited.is_empty());
    assert_eq!(books.owner, i128::from(FUNDED) + 1_000_000_000);
}

// A peer's address goes when its settlement channel closes
// (spec/decisions/closing-channels.md): a Payment from it after that is
// the owner's deposit, as from any address no peer gave. The state version
// changes with it, though the balance does not, so the node writes the
// state without the address and a restart does not bring it back.
#[test]
fn a_closed_peers_address_is_forgotten() {
    let (mut world, _, state_b) = funded_pair();
    let peer = world.nodes[1].name;
    let address = world.nodes[1].scheme.account().to_address();
    world.settle(1, 0, 1_000_000_000, &state_b);
    world.close();
    assert_eq!(world.nodes[0].books.peers.get(&peer), Some(&1_000_000_000));
    let (version, balance) = (
        world.nodes[0].scheme.state_version(),
        world.nodes[0].scheme.balance(),
    );
    world.nodes[0].scheme.on_closed(peer);
    world.nodes[0].scheme.check_invariants();
    assert_eq!(world.nodes[0].scheme.balance(), balance);
    assert_ne!(world.nodes[0].scheme.state_version(), version);
    let state = String::from_utf8(world.nodes[0].scheme.state()).unwrap();
    assert!(!state.contains(&address), "{state}");
    // A channel closed again changes nothing more.
    let version = world.nodes[0].scheme.state_version();
    world.nodes[0].scheme.on_closed(peer);
    assert_eq!(world.nodes[0].scheme.state_version(), version);
    world.settle(1, 0, 2_000_000_000, &state_b);
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.peers.get(&peer), Some(&1_000_000_000));
    assert_eq!(books.owner, i128::from(FUNDED) + 2_000_000_000);
}

// Peering attempts that never became peerings, ours and others', as many
// as the scheme keeps addresses, take no peer's place: the peer's next
// settlement is still its own, not the owner's deposit.
#[test]
fn peering_attempts_never_displace_a_peers_address() {
    let (mut world, _, state_b) = funded_pair();
    let peer = world.nodes[1].name;
    let mut info = Vec::new();
    world.nodes[1].scheme.peering_info(&mut info);
    for i in 0..dassie_xrpl::scheme::MAX_PEERS {
        let stranger = name(&format!("stranger{i}"));
        let (mut request, mut response) = (Vec::new(), Vec::new());
        let scheme = &mut world.nodes[0].scheme;
        scheme
            .create_peering_request(stranger, &info, &mut request)
            .unwrap();
        scheme
            .accept_peering_request(stranger, &info, &mut response)
            .unwrap();
        scheme.check_invariants();
    }
    world.settle(1, 0, 3_000_000_000, &state_b);
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(books.peers.get(&peer), Some(&3_000_000_000));
}

/// The least entry payment the nodes here take: one largest packet at the
/// node's default, 0.1 XRP, in internal units.
const ENTRY: u64 = 100_000_000;

/// The request data node `index` sends node `to` when it asks to peer: its
/// own address.
fn request_of(world: &mut World, index: usize, to: usize) -> Vec<u8> {
    let mut info = Vec::new();
    world.nodes[to].scheme.peering_info(&mut info);
    let mut request = Vec::new();
    let peer = world.nodes[to].name;
    world.nodes[index]
        .scheme
        .create_peering_request(peer, &info, &mut request)
        .unwrap();
    request
}

/// Node `from` pays node `to` `amount` before they peer, as it would
/// settle with it: from its address, with the scheme's memo naming itself.
fn pay_to_enter(world: &mut World, from: usize, to: usize, amount: u64) {
    let mut info = Vec::new();
    world.nodes[to].scheme.peering_info(&mut info);
    world.settle(from, to, amount, &info);
}

/// Node `index` books what it held from `peer`, just peered.
fn take_entry(world: &mut World, index: usize, peer: Name) {
    let mut effects = Vec::new();
    world.nodes[index].scheme.take_entry(peer, &mut effects);
    world.nodes[index].scheme.check_invariants();
    world.apply(index, effects);
    world.check();
}

// spec/decisions/pay-to-enter.md: a Payment with our memo from a node not
// yet peered, of at least one largest packet, is held off the books once
// validated, bound to the node it names and the address it came from. Once
// the node peers from that address, it is booked as its settlement, once,
// and is no longer held.
#[test]
fn a_payment_before_peering_is_held_until_its_node_peers() {
    let mut world = World::new(2);
    world.closes(2);
    world.nodes[0].scheme.take_entries_from(ENTRY);
    let newcomer = world.nodes[1].name;
    let request = request_of(&mut world, 1, 0);
    let entry = |world: &World, node: Name, data: &[u8]| world.nodes[0].scheme.entry(node, data);
    assert_eq!(entry(&world, newcomer, &request), EntryPayment::Missing);
    pay_to_enter(&mut world, 1, 0, ENTRY);
    assert_eq!(
        entry(&world, newcomer, &request),
        EntryPayment::Missing,
        "not validated yet"
    );
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED), "no deposit");
    assert!(books.credited.is_empty());
    assert_eq!(world.nodes[0].scheme.entries_held(), ENTRY);
    assert_eq!(entry(&world, newcomer, &request), EntryPayment::Held);
    // Bound to the node its memo names, and to the address it came from.
    assert_eq!(
        entry(&world, name("node9"), &request),
        EntryPayment::Missing
    );
    let elsewhere = request_of(&mut world, 0, 1);
    assert_eq!(entry(&world, newcomer, &elsewhere), EntryPayment::Missing);
    world.peer(1, 0);
    take_entry(&mut world, 0, newcomer);
    let books = &world.nodes[0].books;
    assert_eq!(books.peers[&newcomer], i128::from(ENTRY));
    assert_eq!(books.credited.len(), 1);
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(world.nodes[0].scheme.entries_held(), 0);
    // Used: it admits no one again, and books nothing more.
    assert_eq!(entry(&world, newcomer, &request), EntryPayment::Missing);
    take_entry(&mut world, 0, newcomer);
    world.closes(2);
    assert_eq!(world.nodes[0].books.credited.len(), 1);
    assert_eq!(world.nodes[0].books.peers[&newcomer], i128::from(ENTRY));
}

// spec/decisions/pay-to-enter.md: what is less than one largest packet is
// no entry payment but the owner's deposit at once; a payment to another
// address is not ours to hold; and one held that its node never claims is
// the owner's deposit once its hold is over.
#[test]
fn an_entry_payment_is_held_only_when_it_counts() {
    let mut world = World::new(3);
    world.closes(2);
    world.nodes[0].scheme.take_entries_from(ENTRY);
    let newcomer = world.nodes[1].name;
    let request = request_of(&mut world, 1, 0);
    pay_to_enter(&mut world, 1, 0, ENTRY - UNITS_PER_DROP);
    pay_to_enter(&mut world, 1, 2, ENTRY);
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED + ENTRY - UNITS_PER_DROP));
    assert_eq!(world.nodes[0].scheme.entries_held(), 0);
    assert_eq!(
        world.nodes[0].scheme.entry(newcomer, &request),
        EntryPayment::Missing
    );
    pay_to_enter(&mut world, 1, 0, ENTRY);
    world.close();
    assert_eq!(
        world.nodes[0].scheme.entry(newcomer, &request),
        EntryPayment::Held
    );
    world.pass(ENTRY_HOLD_LEDGERS);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(world.nodes[0].scheme.entries_held(), 0);
    assert_eq!(books.owner, i128::from(FUNDED + 2 * ENTRY - UNITS_PER_DROP));
    assert!(books.credited.is_empty());
    assert_eq!(
        world.nodes[0].scheme.entry(newcomer, &request),
        EntryPayment::Missing
    );
}

// spec/decisions/pay-to-enter.md: an entry payment held is kept with the
// scheme's state, so a restart before its node peers still holds it, and
// one after books it no second time.
#[test]
fn an_entry_payment_survives_a_restart_and_is_booked_once() {
    let mut world = World::new(2);
    world.closes(2);
    world.nodes[0].scheme.take_entries_from(ENTRY);
    let newcomer = world.nodes[1].name;
    let request = request_of(&mut world, 1, 0);
    pay_to_enter(&mut world, 1, 0, ENTRY);
    world.close();
    let restart = |world: &mut World| {
        let state = world.nodes[0].scheme.state();
        let checkpoint = world.nodes[0].scheme.balance();
        world.disconnect(0);
        let node = world.nodes[0].name;
        world.nodes[0].scheme =
            XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
        world.nodes[0].scheme.take_entries_from(ENTRY);
        world.connect(0);
        world.closes(2);
    };
    restart(&mut world);
    assert_eq!(world.nodes[0].scheme.entries_held(), ENTRY);
    assert_eq!(
        world.nodes[0].scheme.entry(newcomer, &request),
        EntryPayment::Held
    );
    world.peer(1, 0);
    take_entry(&mut world, 0, newcomer);
    restart(&mut world);
    let books = &world.nodes[0].books;
    assert_eq!(books.credited.len(), 1);
    assert_eq!(books.peers[&newcomer], i128::from(ENTRY));
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(world.nodes[0].scheme.entries_held(), 0);
}

/// Two synced nodes, node 0 taking entry payments and holding one from node
/// 1, its account then raised a reserve so high that what it holds above the
/// reserve, `left` drops, is less than the entry held: the reserve took part
/// of it, as fees or the owner's spending outside the node would.
fn drained_with_entry(left: u64) -> (World, Vec<u8>) {
    let mut world = World::new(2);
    world.closes(2);
    world.nodes[0].scheme.take_entries_from(ENTRY);
    let request = request_of(&mut world, 1, 0);
    pay_to_enter(&mut world, 1, 0, ENTRY);
    world.close();
    assert_eq!(world.nodes[0].scheme.entries_held(), ENTRY);
    let balance = world.ledger.balance(world.account(0)).unwrap();
    world.ledger.set_reserve(balance - left, 0);
    world.close();
    assert_eq!(world.nodes[0].scheme.balance(), 0, "the books hold nothing");
    (world, request)
}

/// Node 0 restarts from what its books kept, and catches up.
fn restart_first(world: &mut World) {
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.nodes[0].scheme.take_entries_from(ENTRY);
    world.connect(0);
    world.closes(2);
}

// spec/decisions/pay-to-enter.md: an entry payment taken once the account no
// longer holds all of it above the reserve credits its node what it paid, and
// the books match what is left, the shortfall through suspense, so they still
// match the account, before and after a restart.
#[test]
fn an_entry_payment_taken_short_is_matched_through_suspense() {
    const LEFT: u64 = 40_000;
    let (mut world, _) = drained_with_entry(LEFT);
    let newcomer = world.nodes[1].name;
    world.peer(1, 0);
    take_entry(&mut world, 0, newcomer);
    let books = &world.nodes[0].books;
    assert_eq!(books.peers[&newcomer], i128::from(ENTRY));
    assert_eq!(books.settlement, i128::from(LEFT * UNITS_PER_DROP));
    assert_eq!(
        books.reconciled.last(),
        Some(&(
            -i128::from(ENTRY - LEFT * UNITS_PER_DROP),
            ReconcileCause::Unexplained
        ))
    );
    restart_first(&mut world);
    assert_eq!(
        world.nodes[0].scheme.balance(),
        LEFT * UNITS_PER_DROP,
        "nothing booked again"
    );
    assert_eq!(world.nodes[0].books.credited.len(), 1);
}

// spec/decisions/pay-to-enter.md: an entry payment whose hold is over once
// the account no longer holds all of it above the reserve is the owner's
// deposit, and the books match what is left, so a restart after it reads
// the same.
#[test]
fn an_entry_payment_expired_short_is_matched_through_suspense() {
    const LEFT: u64 = 40_000;
    let (mut world, _) = drained_with_entry(LEFT);
    let owner = world.nodes[0].books.owner;
    world.pass(ENTRY_HOLD_LEDGERS);
    world.closes(2);
    assert_eq!(world.nodes[0].scheme.entries_held(), 0);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, owner + i128::from(ENTRY));
    assert_eq!(books.settlement, i128::from(LEFT * UNITS_PER_DROP));
    restart_first(&mut world);
    assert_eq!(world.nodes[0].scheme.balance(), LEFT * UNITS_PER_DROP);
    assert!(world.nodes[0].books.credited.is_empty());
}

// A Payment the ledger refuses after taking its fee (here: to an account
// that does not exist, too little to create it) is cancelled, its fee
// booked.
#[test]
fn a_failed_settlement_is_cancelled_and_costs_its_fee() {
    let (mut world, _, _) = funded_pair();
    let nobody = dassie_xrpl::Wallet::from_seed([77; 16]).account();
    let mut state = Vec::new();
    let address = nobody.to_address();
    state.push(u8::try_from(address.len()).unwrap());
    state.extend_from_slice(address.as_bytes());
    let id = world.settle(0, 1, 1_000, &state);
    world.close();
    let books = &world.nodes[0].books;
    assert_eq!(books.cancelled, [id]);
    assert_eq!(books.fees, 10 * i128::from(UNITS_PER_DROP));
}

// A settlement whose submission was lost is looked for once past its last
// ledger, and cancelled when the ledger never saw it.
#[test]
fn a_lost_submission_expires_and_is_cancelled() {
    let (mut world, state_a, _) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 2_000_000_000, &state_a);
    world.closes(25);
    assert_eq!(world.nodes[0].books.cancelled, [id]);
    assert!(world.nodes[0].books.pending.is_empty());
    // The sequence was read again: the next one gets through.
    world.nodes[0].drop_submits = false;
    let next = world.settle(0, 1, 2_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [next]);
}

// A settlement signed and kept before a restart is submitted again
// after it, and posted once; what happened while away is booked once,
// with who did it.
#[test]
fn a_restart_resubmits_and_catches_up() {
    let (mut world, state_a, state_b) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    // Node 0 goes down with its settlement signed; its state and balance
    // are what the node kept.
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    // Meanwhile node 1 settles with node 0, and someone deposits.
    world.settle(1, 0, 4_000_000_000, &state_b);
    world.ledger.deposit(world.account(0), 7_000_000);
    world.closes(2);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.nodes[0].drop_submits = false;
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert_eq!(books.credited.len(), 1);
    assert_eq!(books.owner, i128::from(FUNDED) + 7_000_000_000);
    assert_eq!(books.suspense, 0);
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: a server that no
// longer has a ledger within the history bound, which the proven state
// names, keeps too short a history: the account waits, says so, and books
// nothing it cannot prove.
#[test]
fn a_short_history_within_the_bound_stalls() {
    let (mut world, _, state_b) = funded_pair();
    world.disconnect(0);
    world.settle(1, 0, 4_000_000_000, &state_b);
    world.closes(3);
    world.ledger.forget_history();
    world.connect(0);
    world.closes(1);
    let books = &world.nodes[0].books;
    assert!(books.credited.is_empty() && books.reconciled.is_empty());
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
    let waiting = world.nodes[0].scheme.account.waiting().unwrap();
    assert!(waiting.contains("history is too short"), "{waiting}");
}

/// Two funded, synced and peered nodes, the first of which settles `amount`
/// with the second while it is away for longer than the history bound,
/// past the first flag ledger. Its server has forgotten the settlement's
/// ledger.
fn away_beyond_the_bound(amount: u64) -> World {
    let (mut world, _, state_b) = funded_pair();
    world.closes(260);
    world.disconnect(0);
    world.settle(1, 0, amount, &state_b);
    world.closes(3);
    world.disconnect(1);
    world.pass(HISTORY_BOUND + 300);
    world
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: history the
// server no longer has, beyond the history bound as the proven state
// names it, is history it does not have: the balance says what changed,
// reconciled through suspense, exactly, and the gap is counted.
#[test]
fn history_beyond_the_bound_reconciles_by_the_balance() {
    let mut world = away_beyond_the_bound(4_000_000_000);
    world.connect(0);
    world.closes(1);
    assert_eq!(world.nodes[0].scheme.account.unproven().history_gaps, 1);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    assert_eq!(world.nodes[0].books.suspense, 4_000_000_000);
    assert!(world.nodes[0].books.credited.is_empty());
    // Nothing the scheme can name: a warning.
    assert_eq!(
        world.nodes[0].books.reconciled,
        [(4_000_000_000, ReconcileCause::Unexplained)]
    );
}

// A reserve change moves what is spendable: reconciled at once, as the
// reserve's, which is expected.
#[test]
fn a_new_reserve_is_reconciled() {
    let (mut world, _, _) = funded_pair();
    assert!(world.nodes[0].books.reconciled.is_empty());
    world.ledger.set_reserve(2 * RESERVE_BASE, 0);
    world.close();
    let reserve = i128::from(RESERVE_BASE * UNITS_PER_DROP);
    assert_eq!(world.nodes[0].books.suspense, -reserve);
    assert_eq!(
        world.nodes[0].books.reconciled,
        [(-reserve, ReconcileCause::Reserve)]
    );
}

// A reserve above the balance explains what the books hold, and no more:
// they never hold less than nothing. Lowered again, it explains what comes
// back.
#[test]
fn a_reserve_above_the_balance_explains_down_to_nothing() {
    let (mut world, _, _) = funded_pair();
    let held = i128::from(world.nodes[0].scheme.balance());
    assert!(held > 0);
    world.ledger.set_reserve(2 * FAUCET_DROPS, 0);
    world.close();
    assert_eq!(world.nodes[0].scheme.balance(), 0);
    world.ledger.set_reserve(RESERVE_BASE, 0);
    world.close();
    assert_eq!(
        world.nodes[0].books.reconciled,
        [
            (-held, ReconcileCause::Reserve),
            (held, ReconcileCause::Reserve)
        ]
    );
}

// A reserve change and a difference the scheme cannot name, met in one
// match: each is reconciled for its own part, and together they make the
// books match the balance.
#[test]
fn a_reserve_change_is_told_from_an_unexplained_difference() {
    let mut world = away_beyond_the_bound(4_000_000_000);
    world.ledger.set_reserve(2 * RESERVE_BASE, 0);
    world.close();
    world.connect(0);
    world.closes(1);
    let reserve = i128::from(RESERVE_BASE * UNITS_PER_DROP);
    let books = &world.nodes[0].books;
    assert_eq!(books.suspense, 4_000_000_000 - reserve);
    let mut reconciled = books.reconciled.clone();
    reconciled.sort_by_key(|(amount, _)| *amount);
    assert_eq!(
        reconciled,
        [
            (-reserve, ReconcileCause::Reserve),
            (4_000_000_000, ReconcileCause::Unexplained)
        ]
    );
}

// A testnet reset takes the funds: booked as a loss, and the faucet asked
// again, whose funds are a deposit.
#[test]
fn a_testnet_reset_is_a_loss_and_a_new_deposit() {
    let (mut world, _, _) = funded_pair();
    world.ledger.reset();
    world.disconnect(0);
    world.connect(0);
    // The faucet is asked at most once a minute.
    world.closes(20);
    let books = &world.nodes[0].books;
    assert_eq!(books.suspense, -i128::from(FUNDED));
    assert_eq!(
        books.reconciled,
        [(-i128::from(FUNDED), ReconcileCause::Unexplained)]
    );
    assert_eq!(books.owner, 2 * i128::from(FUNDED));
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// The owner may ask the faucet for more, at its pace: what it sends is a
// deposit once the ledger shows it, never booked at the asking.
#[test]
fn the_owner_asks_the_faucet_for_more() {
    let (mut world, _, _) = funded_pair();
    assert!(world.nodes[0].scheme.has_faucet());
    let ask = |world: &mut World| {
        let mut effects = Vec::new();
        let asked = world.nodes[0].scheme.ask_faucet(&mut effects);
        world.nodes[0].scheme.check_invariants();
        (asked, effects)
    };
    // Asked when the account was made, less than a minute ago.
    assert_eq!(ask(&mut world), (Err(SchemeError::Unavailable), Vec::new()));
    world.closes(15);
    let (asked, effects) = ask(&mut world);
    assert_eq!(asked, Ok(()));
    assert!(matches!(effects.as_slice(), [Effect::Fund { .. }]));
    assert_eq!(world.nodes[0].books.owner, i128::from(FUNDED));
    world.apply(0, effects);
    assert_eq!(ask(&mut world).0, Err(SchemeError::Unavailable));
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(
        books.owner,
        i128::from(FUNDED) + i128::from(FAUCET_DROPS * UNITS_PER_DROP)
    );
    assert_eq!(books.suspense, 0);
    // Not while there is no connection to take its answer on.
    world.closes(15);
    world.disconnect(0);
    assert_eq!(ask(&mut world).0, Err(SchemeError::Unavailable));
}

#[test]
fn a_ledger_without_a_faucet_is_never_asked() {
    let mut scheme =
        XrplScheme::new(r#"{"faucet":null}"#, &entropy(1), name("node0"), None, &[]).unwrap();
    assert!(!scheme.has_faucet());
    let mut effects = Vec::new();
    assert_eq!(
        scheme.ask_faucet(&mut effects),
        Err(SchemeError::Unsupported)
    );
    assert!(effects.is_empty());
}

// spec/decisions/synced-before-peering.md: the scheme has synced once its
// first sync booked what the account held as the owner's, and the node peers
// on it only from then. That is kept with its state, so a restart that cannot
// reach the ledger keeps it.
#[test]
fn the_first_sync_is_kept_across_restarts() {
    let node = name("node0");
    let fresh = XrplScheme::new("", &entropy(1), node, None, &[]).unwrap();
    assert!(!fresh.has_synced(), "synced before it saw its ledger");
    let mut world = World::new(1);
    world.closes(2);
    assert!(world.nodes[0].scheme.has_synced(), "never synced");
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let restarted = XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    assert_eq!(restarted.status(), LedgerStatus::Connecting);
    assert!(restarted.has_synced(), "a restart forgot the first sync");
}

// spec/decisions/ledger-resets.md: a reset starts the chain again from a
// ledger below the newest seen, which closed later. It is recognised as one
// even when the account exists again on the new chain: the old funds are a
// loss, the new a deposit.
#[test]
fn a_reset_is_recognised_when_the_account_exists_again() {
    let (mut world, _, _) = funded_pair();
    world.disconnect(0);
    world.ledger.reset();
    world.ledger.fund(world.account(0));
    world.close();
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.suspense, -i128::from(FUNDED));
    assert_eq!(books.owner, 2 * i128::from(FUNDED));
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// Found in review: after a reconnect, a server says the account does not
// exist, while a settlement of ours applied. An absence counts only when
// proven: nothing is cancelled or written off, and once the server is
// honest the settlement is posted.
#[test]
fn an_absence_the_server_made_up_cancels_nothing() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 5_000_000_000, &state_a);
    world.disconnect(0);
    world.close();
    assert!(world.nodes[1].books.credited.contains(&id));
    let write_offs = world.nodes[0].scheme.account.write_offs();
    world.ledger.set_lie_for_all(Some(Lie::FalseAbsence));
    world.connect(0);
    world.closes(2);
    world.ledger.set_lie_for_all(None);
    assert!(world.ledger.lies_told(Lie::FalseAbsence) > 0);
    let books = &world.nodes[0].books;
    assert!(books.cancelled.is_empty() && books.reconciled.is_empty());
    assert_eq!(world.nodes[0].scheme.account.write_offs(), write_offs);
    world.wait(STALL_MS);
    world.closes(2);
    assert_eq!(world.nodes[0].books.finalized, [id]);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// As above, against a server that stopped at the first ledger, before the
// account was made: it is never believed, and the scheme waits. Once a
// server has caught up, the settlement is posted.
#[test]
fn a_server_that_never_saw_the_account_writes_nothing_off() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 5_000_000_000, &state_a);
    world.disconnect(0);
    world.close();
    world.nodes[0].behind = Some(FakeLedger::new());
    world.connect(0);
    // Long enough to reconnect when the stopped server goes silent.
    world.closes(10);
    assert!(world.nodes[0].reconnects > 0);
    let books = &world.nodes[0].books;
    assert!(books.cancelled.is_empty());
    assert_eq!(books.suspense, 0);
    assert!(books.pending.contains_key(&id));
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
    world.nodes[0].behind = None;
    world.disconnect(0);
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert_eq!(books.suspense, 0);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// A settlement never executed is dropped past its last ledger, and its
// sequence is the next one's: the account's sequence as the anchor proves
// it, so the next settlement applies.
#[test]
fn a_dropped_settlement_s_sequence_is_used_again() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 1_000_000_000, &state_a)
        .unwrap();
    world.closes(25);
    let next = world.settle(0, 1, 1_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [next]);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// spec/decisions/ledger-resets.md: a server behind what the scheme booked
// (here a peer's settlement), or only behind the newest ledger it saw, says
// nothing new: nothing is booked from it, and the scheme waits for one that
// caught up.
#[test]
fn a_server_behind_is_never_booked_from() {
    for settled in [true, false] {
        let (mut world, _, state_b) = funded_pair();
        let behind = world.ledger.clone();
        if settled {
            world.settle(1, 0, 4_000_000_000, &state_b);
        }
        world.closes(3);
        let books = |world: &World| {
            let books = &world.nodes[0].books;
            (
                books.settlement,
                books.owner,
                books.suspense,
                books.credited.len(),
            )
        };
        let before = books(&world);
        world.disconnect(0);
        world.nodes[0].behind = Some(behind);
        world.connect(0);
        world.closes(3);
        assert_eq!(books(&world), before, "{settled}");
        assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Syncing);
        world.nodes[0].behind = None;
        world.disconnect(0);
        world.connect(0);
        world.closes(2);
        assert_eq!(books(&world), before, "{settled}");
        assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    }
}

// The settlement message carries the transaction's hash, and books
// nothing: the ledger does.
#[test]
fn the_settlement_claim_books_nothing() {
    let (mut world, _, _) = funded_pair();
    let mut effects = Vec::new();
    let peer = world.nodes[1].name;
    world.nodes[0]
        .scheme
        .handle_settlement(peer, 10, &[1; 32], &[], &mut effects)
        .unwrap();
    assert!(effects.is_empty());
    assert!(
        world.nodes[0]
            .scheme
            .handle_settlement(peer, 10, &[1; 31], &[], &mut effects)
            .is_err()
    );
    let _ = MEMO_TYPE;
}

// Settlements wait while the scheme cannot settle: before it syncs, under
// a drop, or beyond what it holds above the reserve.
#[test]
fn a_settlement_waits_when_it_cannot_be_made() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    let scheme = &mut world.nodes[0].scheme;
    for amount in [999, FUNDED + UNITS_PER_DROP] {
        assert_eq!(
            scheme
                .prepare_settlement(&mut rng, peer, amount, &state_a)
                .err(),
            Some(dassie_settlement::SchemeError::Unavailable),
            "{amount}"
        );
    }
    world.disconnect(0);
    assert_eq!(
        world.nodes[0]
            .scheme
            .prepare_settlement(&mut rng, peer, 1_000_000, &state_a)
            .err(),
        Some(dassie_settlement::SchemeError::Unavailable)
    );
}

// Several settlements in flight at once, to several peers, with
// consecutive sequences: all get in.
#[test]
fn settlements_go_several_at_once() {
    let mut world = World::new(4);
    world.closes(2);
    let states: Vec<Vec<u8>> = (1..4).map(|b| world.peer(0, b).0).collect();
    let ids: Vec<Name> = states
        .iter()
        .enumerate()
        .map(|(i, state)| world.settle(0, i + 1, 1_000_000_000, state))
        .collect();
    world.close();
    assert_eq!(world.nodes[0].books.finalized, ids);
    for b in 1..4 {
        assert_eq!(world.nodes[b].books.credited.len(), 1);
    }
}

// Found in review: a settlement of ours that the ledger validated while the
// node was away, with an account history too short to read it from, is
// still posted, once looked up by its hash: what the balance booked
// through suspense comes back out of it.
#[test]
fn a_settlement_validated_while_away_is_still_posted() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    world.closes(2);
    world.ledger.shorten_account_history();
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.connect(0);
    world.closes(40);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert!(books.pending.is_empty());
    assert_eq!(books.suspense, 0, "the suspense entry came back out");
}

// As above, with a settlement and fee larger than what the account holds
// after it: posting them takes the books below zero, and what they are
// short comes from suspense, so the books still match the balance.
#[test]
fn a_large_settlement_validated_while_away_keeps_the_books() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 700_000_000_000, &state_a);
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    world.closes(2);
    world.ledger.shorten_account_history();
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    world.connect(0);
    world.closes(40);
    let books = &world.nodes[0].books;
    assert_eq!(books.finalized, [id]);
    assert!(books.pending.is_empty());
    assert_eq!(books.suspense, 0, "the suspense entry came back out");
}

// Found in review: a settlement the node prepared but could not book, so
// never executed, is never sent, even across a reconnect.
#[test]
fn a_settlement_never_executed_is_never_sent() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 3_000_000_000, &state_a)
        .unwrap();
    world.disconnect(0);
    world.connect(0);
    world.closes(25);
    assert_eq!(world.nodes[1].books.credited.len(), 0, "nothing was paid");
    // One discarded at once frees its sequence for the next.
    let prepared = world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 1_000_000_000, &state_a)
        .unwrap();
    world.nodes[0].scheme.discard_settlement(prepared.id);
    let id = world.settle(0, 1, 1_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [id]);
}

// spec/areas/xrpl.md#state-and-restarts: a settlement signed and booked,
// but never executed (the node stopped first), is dropped past its last
// ledger, and the account remembers it as never sent. The scheme that
// reconciles it after a restart cancels it, rather than looking for a hash
// no ledger will ever hold and leaving it pending for good.
#[test]
fn a_settlement_never_sent_is_cancelled_after_a_restart() {
    let (mut world, state_a, _) = funded_pair();
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    let prepared = world.nodes[0]
        .scheme
        .prepare_settlement(&mut rng, peer, 3_000_000_000, &state_a)
        .unwrap();
    world.nodes[0]
        .books
        .pending
        .insert(prepared.id, (peer, prepared.amount));
    world.closes(25);
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(prepared.id, prepared.amount)], &mut effects);
    assert_eq!(
        effects,
        [Effect::Cancel {
            settlement: prepared.id
        }]
    );
    world.apply(0, effects);
    world.nodes[0].proved.clear();
    world.connect(0);
    world.closes(2);
    let books = &world.nodes[0].books;
    assert_eq!(books.cancelled, [prepared.id]);
    assert!(books.pending.is_empty());
    // What it proved is the history the walk found, which the ledger
    // holds: never a hash no ledger will.
    for hash in world.nodes[0].proved.clone() {
        let request = serde_json::json!({ "command": "tx", "transaction": hash });
        let reply = world
            .ledger
            .request(99, &serde_json::to_vec(&request).unwrap());
        let reply: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        assert_eq!(
            reply["status"], "success",
            "looked for a transaction never sent"
        );
    }
}

// Found in review: a connection that goes quiet is made anew, rather than
// leaving the scheme synced to a ledger it no longer hears.
#[test]
fn a_silent_connection_is_made_anew() {
    let (mut world, _, _) = funded_pair();
    // The ledger forgets the connection; the scheme is not told.
    let connection = world.nodes[0].connection;
    world.ledger.disconnect(connection);
    world.nodes[0].online = false;
    world.now += 31_000;
    let mut effects = Vec::new();
    world.nodes[0].scheme.on_timer(world.now, &mut effects);
    assert!(effects.contains(&Effect::Reconnect), "{effects:?}");
    world.apply(0, effects);
    world.closes(2);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
}

// Found in review: after a testnet reset, what was in flight can never
// apply: it is cancelled, and settling starts anew.
#[test]
fn a_reset_cancels_what_was_in_flight() {
    let (mut world, state_a, _) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    world.ledger.reset();
    world.disconnect(0);
    world.connect(0);
    world.closes(20);
    assert_eq!(world.nodes[0].books.cancelled, [id]);
    world.nodes[0].drop_submits = false;
    let next = world.settle(0, 1, 1_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [next]);
}

// Found in review: a settlement the node held pending across a restart
// though the scheme had settled it (its posting waited for an earlier
// announcement) is looked up by its hash, and posted.
#[test]
fn a_pending_settlement_unknown_to_the_scheme_is_recovered() {
    let (mut world, state_a, _) = funded_pair();
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    world.close();
    // The scheme settled it; the node did not post it.
    let finalized = world.nodes[0].books.finalized.pop().unwrap();
    assert_eq!(finalized, id);
    let peer = world.nodes[1].name;
    let books = &mut world.nodes[0].books;
    books.pending.insert(id, (peer, 3_000_000_000));
    books.settlement += 3_000_000_000;
    *books.peers.get_mut(&peer).unwrap() += 3_000_000_000;
    // A restart hands the scheme what the node holds pending.
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(id, 3_000_000_000)], &mut effects);
    world.connect(0);
    world.closes(2);
    assert_eq!(world.nodes[0].books.finalized, [id]);
    assert!(world.nodes[0].books.pending.is_empty());
}

// A settlement left pending across a restart that no history holds cannot
// be known to have applied: it stays pending, for the owner
// (spec/areas/xrpl.md#state-and-restarts). It is asked again less and less
// often, not at every ledger.
#[test]
fn a_settlement_no_history_holds_stays_pending_and_is_asked_less_often() {
    let (mut world, _, _) = funded_pair();
    let id = Name::hex(&[7; 32], true);
    let peer = world.nodes[1].name;
    world.nodes[0]
        .books
        .pending
        .insert(id, (peer, 1_000_000_000));
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let mut effects = Vec::new();
    world.nodes[0]
        .scheme
        .reconcile(&[(id, 1_000_000_000)], &mut effects);
    let lookups = |world: &World| {
        let looked_up = dassie_xrpl::hex_upper(&[7; 32]);
        let proved = world.nodes[0].proved.iter();
        proved.filter(|hash| **hash == looked_up).count()
    };
    world.connect(0);
    world.closes(600);
    // Asked at once, then after 1, 2, 4 … 256 ledgers: 10 times in 600.
    assert_eq!(lookups(&world), 10);
    world.closes(512);
    assert_eq!(lookups(&world), 12, "asked every 256 ledgers at most");
    let books = &world.nodes[0].books;
    assert!(books.finalized.is_empty() && books.cancelled.is_empty());
    assert!(books.pending.contains_key(&id));
}

// spec/decisions/tigerstyle.md: a server that reports more XRP for the
// account than the books can hold stops the scheme, rather than booking an
// amount clamped to fit.
#[test]
fn more_xrp_than_the_books_hold_stops_the_scheme() {
    let (mut world, _, _) = funded_pair();
    let owner = world.nodes[0].books.owner;
    let account = world.account(0);
    world.ledger.deposit(account, dassie_xrpl::MAX_DROPS / 10);
    world.closes(2);
    assert!(matches!(
        world.nodes[0].scheme.status(),
        LedgerStatus::Failed(_)
    ));
    assert_eq!(world.nodes[0].books.owner, owner);
    assert_eq!(world.nodes[0].scheme.deadline(), None);
    assert_eq!(world.nodes[1].scheme.status(), LedgerStatus::Synced);
}

// Found by the invariants (spec/decisions/tigerstyle.md): more settlements to
// look up after a restart than requests may wait made the scheme reconnect,
// and then look them all up again, for ever. It asks for a few at a time.
#[test]
fn many_settlements_to_recover_are_looked_up_a_few_at_a_time() {
    let (mut world, _, _) = funded_pair();
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    let pending: Vec<(Name, u64)> = (0..20)
        .map(|byte| (Name::hex(&[byte; 32], true), 1_000_000_000))
        .collect();
    let mut effects = Vec::new();
    world.nodes[0].scheme.reconcile(&pending, &mut effects);
    world.nodes[0].hold = Some("tx_proof");
    let before = world.nodes[0].proved.len();
    world.connect(0);
    world.closes(2);
    assert_eq!(world.nodes[0].scheme.status(), LedgerStatus::Synced);
    assert_eq!(world.nodes[0].reconnects, 0);
    let looked_up: Vec<String> = (0..20)
        .map(|byte| dassie_xrpl::hex_upper(&[byte; 32]))
        .collect();
    let asked = world.nodes[0].proved.iter().skip(before);
    assert_eq!(asked.filter(|hash| looked_up.contains(hash)).count(), 8);
}

// An account at the last sequence there is can send nothing more: a
// settlement waits rather than reuse it.
#[test]
fn the_last_sequence_is_never_reused() {
    let (mut world, state_a, _) = funded_pair();
    let account = world.account(0);
    world.ledger.set_sequence(account, u32::MAX);
    world.close();
    world.disconnect(0);
    world.connect(0);
    let peer = world.nodes[1].name;
    let mut rng = SeededRng::new(1);
    let prepared =
        world.nodes[0]
            .scheme
            .prepare_settlement(&mut rng, peer, 1_000_000_000, &state_a);
    assert_eq!(prepared.map(|_| ()), Err(SchemeError::Unavailable));
}

/// Whether node `index`'s scheme stopped.
fn failed(world: &World, index: usize) -> bool {
    matches!(world.nodes[index].scheme.status(), LedgerStatus::Failed(_))
}

// The network is checked first: the first request on a connection is
// `server_state`, and the account is subscribed only after it, then
// anchored on a ledger its UNL validated, which proves it: it is never
// read on the server's word
// (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
#[test]
fn the_network_is_checked_before_the_account_is_read() {
    let (world, _, _) = funded_pair();
    let asked = &world.nodes[0].asked;
    assert_eq!(
        asked[..4],
        ["server_state", "subscribe", "state_proof", "validations"]
    );
    // `tx` gives a transaction's bytes only once its proof came, which
    // they must then prove (`World::request`).
    for read in ["account_info", "ledger_entry", "amm_info"] {
        assert!(!asked.iter().any(|asked| asked == read), "{read}");
    }
}

// A server off the XRP Ledger testnet (mainnet 0, devnet 2, a network
// above 1024), or one that names no network, stops the scheme before it
// reads the account: the faucet is never asked, nothing is booked or
// written off, and nothing can be signed.
#[test]
fn a_server_off_the_testnet_stops_the_scheme() {
    for network in [Some(0), Some(2), Some(21_338), None] {
        let mut ledger = FakeLedger::new();
        ledger.set_network_id(network);
        let mut world = World::on(ledger, 2);
        world.closes(20);
        let peer = world.nodes[1].name;
        let (state, _) = world.peer(0, 1);
        for index in 0..2 {
            assert!(failed(&world, index), "{network:?}");
            assert_eq!(world.nodes[index].asked, ["server_state"], "{network:?}");
            // The faucet would have made the account.
            assert_eq!(world.ledger.balance(world.account(index)), None);
            let books = &world.nodes[index].books;
            assert_eq!(
                (books.settlement, books.owner, books.suspense, books.fees),
                (0, 0, 0, 0),
                "{network:?}"
            );
            assert_eq!(world.nodes[index].scheme.deadline(), None);
        }
        let mut rng = SeededRng::new(1);
        let prepared =
            world.nodes[0]
                .scheme
                .prepare_settlement(&mut rng, peer, 1_000_000_000, &state);
        assert_eq!(
            prepared.err(),
            Some(SchemeError::Unavailable),
            "{network:?}"
        );
    }
}

// A synced scheme whose server is then off the testnet stops when it
// connects again: nothing is written off, and the books stay as they were.
// The other node, on its own connection, goes on.
#[test]
fn a_synced_scheme_whose_server_leaves_the_testnet_stops() {
    let (mut world, _, _) = funded_pair();
    let before = (world.nodes[0].books.owner, world.nodes[0].books.settlement);
    world.ledger.set_network_id(Some(0));
    world.disconnect(0);
    world.connect(0);
    world.closes(20);
    assert!(failed(&world, 0));
    let books = &world.nodes[0].books;
    assert_eq!((books.owner, books.settlement), before);
    assert_eq!(books.suspense, 0);
    assert_eq!(world.nodes[0].scheme.balance(), FUNDED);
    assert_eq!(world.nodes[0].scheme.deadline(), None);
    assert_eq!(world.nodes[1].scheme.status(), LedgerStatus::Synced);
}

/// What the ledger holds under `hash`, validated.
fn validated(world: &mut World, hash: &str) -> dassie_xrpl::api::TxRecord {
    let hash: [u8; 32] = dassie_xrpl::from_hex(hash).unwrap().try_into().unwrap();
    let reply = world
        .ledger
        .request(999, &dassie_xrpl::api::tx_anywhere(1, &hash));
    let outcome = match dassie_xrpl::api::parse(&reply) {
        dassie_xrpl::api::Message::Response { outcome, .. } => Some(outcome),
        _ => None,
    };
    dassie_xrpl::api::tx_result(&outcome.unwrap().unwrap()).unwrap()
}

/// The transaction of the last fee node `index` booked.
fn submitted_hash(world: &World, index: usize) -> String {
    let fee_ids: Vec<&Name> = world.nodes[index].books.fee_ids.iter().collect();
    fee_ids.last().unwrap().as_str().to_string()
}

// spec/areas/xrpl.md#deposits-and-withdrawals: the owner pays out of the
// account, to a destination with a tag and one memo, as a deposit into a
// settlement hub is made. Once the ledger validated it, it is the owner's
// withdrawal, with its fee; the account it reached books it as a deposit,
// as it books any payment from an address no peer gave. One at a time.
#[test]
fn the_owner_pays_out_of_the_account() {
    use dassie_settlement::{OwnerPayment, OwnerPaymentState};

    let (mut world, _, _) = funded_pair();
    let to = world.account(1);
    let payment = OwnerPayment {
        destination: to.to_address(),
        tag: Some(42),
        memo: Some(vec![7; 32]),
        amount: 3_000_000_999,
    };
    let mut effects = Vec::new();
    let id = world.pay_owner(0, &payment, &mut effects).unwrap();
    assert_eq!(
        world.nodes[0].scheme.owner_payment(id),
        OwnerPaymentState::InFlight
    );
    assert_eq!(
        world.pay_owner(0, &payment, &mut Vec::new()),
        Err(SchemeError::Unavailable),
        "one at a time"
    );
    world.apply(0, effects);
    world.close();
    assert_eq!(
        world.nodes[0].scheme.owner_payment(id),
        OwnerPaymentState::Applied
    );
    let fee = 10 * i128::from(UNITS_PER_DROP);
    let (a, b) = (&world.nodes[0].books, &world.nodes[1].books);
    assert_eq!(a.owner, i128::from(FUNDED) - 3_000_000_000);
    assert_eq!(a.fees, fee);
    assert_eq!(b.owner, i128::from(FUNDED) + 3_000_000_000);
    assert!(b.credited.is_empty(), "no settlement");
    assert_eq!(a.suspense + b.suspense, 0);
    let hash = submitted_hash(&world, 0);
    assert_eq!(hash, id.as_str(), "its ID is its hash");
    let record = validated(&mut world, &hash);
    assert_eq!(record.destination, Some(to));
    assert_eq!(record.destination_tag, Some(42));
    assert_eq!(record.delivered, Some(3_000_000));
    assert_eq!(
        record.memo,
        Some((dassie_xrpl::scheme::OWNER_MEMO_TYPE.to_vec(), vec![7; 32]))
    );
}

// spec/areas/xrpl.md#deposits-and-withdrawals: the owner's payment on its
// way is kept across a restart and booked once after; what became of it
// outlives the next restart, for the last payment only.
#[test]
fn the_owners_payment_outlives_a_restart() {
    use dassie_settlement::{OwnerPayment, OwnerPaymentState};

    let (mut world, _, _) = funded_pair();
    let payment = OwnerPayment {
        destination: world.account(1).to_address(),
        tag: None,
        memo: None,
        amount: 3_000_000_000,
    };
    let mut effects = Vec::new();
    let id = world.pay_owner(0, &payment, &mut effects).unwrap();
    world.apply(0, effects);
    world.close();
    let fee = 10 * i128::from(UNITS_PER_DROP);
    world.nodes[0].drop_submits = true;
    let second = world.pay_owner(0, &payment, &mut Vec::new()).unwrap();
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.disconnect(0);
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    assert_eq!(
        world.nodes[0].scheme.owner_payment(second),
        OwnerPaymentState::InFlight
    );
    world.nodes[0].drop_submits = false;
    world.connect(0);
    world.closes(2);
    // Its outcome outlives another restart; the one before is no longer
    // the scheme's to tell.
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    assert_eq!(
        world.nodes[0].scheme.owner_payment(second),
        OwnerPaymentState::Applied
    );
    assert_eq!(
        world.nodes[0].scheme.owner_payment(id),
        OwnerPaymentState::Unknown
    );
    let a = &world.nodes[0].books;
    assert_eq!(a.owner, i128::from(FUNDED) - 6_000_000_000);
    assert_eq!(a.fees, 2 * fee);
    assert_eq!(a.suspense, 0);
}

// An owner's payment the ledger refuses costs its fee and nothing else; one
// to the account itself, of nothing, or with too long a memo is refused
// before anything is signed.
#[test]
fn a_failed_owner_payment_costs_only_its_fee() {
    use dassie_settlement::{OwnerPayment, OwnerPaymentState, PaymentFailure};

    let (mut world, _, _) = funded_pair();
    let nobody = dassie_xrpl::Wallet::from_seed([77; 16]).account();
    let payment = |destination: AccountId, amount: u64, memo: usize| OwnerPayment {
        destination: destination.to_address(),
        tag: None,
        memo: (memo > 0).then(|| vec![1; memo]),
        amount,
    };
    let own = world.account(0);
    for refused in [
        payment(own, 1_000_000, 0),
        payment(nobody, 999, 0),
        payment(nobody, 1_000_000, 65),
    ] {
        assert_eq!(
            world.pay_owner(0, &refused, &mut Vec::new()),
            Err(SchemeError::InvalidData)
        );
    }
    // Too little to make a new account.
    let mut effects = Vec::new();
    let id = world
        .pay_owner(0, &payment(nobody, 1_000_000, 0), &mut effects)
        .unwrap();
    world.apply(0, effects);
    world.close();
    // A failure the ledger recorded, too little: never taken for one that
    // never reached the ledger, also after a restart.
    let too_little = OwnerPaymentState::Failed(PaymentFailure::TooLittle);
    assert_eq!(world.nodes[0].scheme.owner_payment(id), too_little);
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(books.fees, 10 * i128::from(UNITS_PER_DROP));
    assert!(books.cancelled.is_empty() && books.finalized.is_empty());
    // Any other failure is told apart from it: to an account that takes
    // only payments with a destination tag, without one.
    let peer = world.account(1);
    let flags = world.ledger.account_flags(peer).unwrap();
    world.ledger.set_account_flags(
        peer,
        flags | dassie_xrpl::api::account_flags::REQUIRE_DEST_TAG,
    );
    let mut effects = Vec::new();
    let other = world
        .pay_owner(0, &payment(peer, 1_000_000, 0), &mut effects)
        .unwrap();
    world.apply(0, effects);
    world.close();
    assert_eq!(
        world.nodes[0].scheme.owner_payment(other),
        OwnerPaymentState::Failed(PaymentFailure::Other)
    );
    // What became of the last outlives a restart.
    let mut effects = Vec::new();
    let again = world
        .pay_owner(0, &payment(nobody, 1_000_000, 0), &mut effects)
        .unwrap();
    world.apply(0, effects);
    world.close();
    let state = world.nodes[0].scheme.state();
    let checkpoint = world.nodes[0].scheme.balance();
    let node = world.nodes[0].name;
    world.nodes[0].scheme =
        XrplScheme::new("", &entropy(1), node, Some(checkpoint), &state).unwrap();
    assert_eq!(world.nodes[0].scheme.owner_payment(again), too_little);
}

// An owner's payment whose submission was lost is looked for once past its
// last ledger and, never seen there, cancelled: it moved nothing, not even
// a fee, which a failure the ledger recorded would have cost.
#[test]
fn a_lost_owners_payment_is_cancelled_not_failed() {
    use dassie_settlement::{OwnerPayment, OwnerPaymentState};

    let (mut world, _, _) = funded_pair();
    world.nodes[0].drop_submits = true;
    let payment = OwnerPayment {
        destination: world.account(1).to_address(),
        tag: None,
        memo: None,
        amount: 3_000_000_000,
    };
    let mut effects = Vec::new();
    let id = world.pay_owner(0, &payment, &mut effects).unwrap();
    world.apply(0, effects);
    world.closes(25);
    assert_eq!(
        world.nodes[0].scheme.owner_payment(id),
        OwnerPaymentState::Cancelled
    );
    let books = &world.nodes[0].books;
    assert_eq!(books.owner, i128::from(FUNDED));
    assert_eq!(books.fees, 0);
}

/// What node `index`'s books say happened: what was credited, finalized and
/// cancelled, what suspense reconciled, and the account's write-offs.
fn outcomes(world: &World, index: usize) -> (Vec<Name>, Vec<Name>, Vec<Name>, usize, u64) {
    let node = &world.nodes[index];
    let credited: Vec<Name> = node.books.credited.iter().copied().collect();
    (
        credited,
        node.books.finalized.clone(),
        node.books.cancelled.clone(),
        node.books.reconciled.len(),
        node.scheme.account.write_offs(),
    )
}

// spec/designs/xrpl-validation.md#1-the-rule: whatever a server lies about,
// in each way the lying fake ledger tells, nothing is booked that the
// ledger did not apply, nothing it applied is cancelled, and nothing is
// written off. A settlement of ours and one of the peer's applied while
// node 0 was away, and it comes back to a server that lies; what it books
// meanwhile, if anything, is what the ledger applied. Once its server is
// honest, it settles both, once, and its books hold the ledger's.
#[test]
fn a_lying_server_books_cancels_and_writes_off_nothing() {
    for lie in Lie::ALL {
        let (mut world, state_a, state_b) = funded_pair();
        // Ledgers enough that some closed long enough ago for their
        // validations to be stale, which an old anchor is.
        world.closes(70);
        let before = outcomes(&world, 0);
        let ours = world.settle(0, 1, 2_000_000_000, &state_a);
        let theirs = world.settle(1, 0, 3_000_000_000, &state_b);
        // And the owner's deposit: three transactions, one in the middle.
        let account = world.account(0);
        world.ledger.deposit(account, 5_000_000);
        world.nodes[0].lagging = true;
        world.disconnect(0);
        world.close();
        let lying = world.next_connection + 1;
        world.ledger.set_lie(lying, Some(lie));
        world.connect(0);
        world.closes(3);
        assert!(world.ledger.lies_told(lie) > 0, "{lie:?} never told");
        let (credited, finalized, cancelled, reconciled, write_offs) = outcomes(&world, 0);
        assert!(
            credited
                .iter()
                .all(|id| *id == theirs || before.0.contains(id)),
            "{lie:?}"
        );
        assert!(finalized.iter().all(|id| *id == ours), "{lie:?}");
        assert_eq!(cancelled, before.2, "{lie:?}");
        assert_eq!((reconciled, write_offs), (before.3, before.4), "{lie:?}");
        assert!(!failed(&world, 0), "{lie:?}");
        world.ledger.set_lie(lying, None);
        world.wait(STALL_MS);
        world.closes(3);
        world.nodes[0].lagging = false;
        world.check();
        assert_eq!(
            world.nodes[0].scheme.status(),
            LedgerStatus::Synced,
            "{lie:?}"
        );
        let (credited, finalized, cancelled, reconciled, write_offs) = outcomes(&world, 0);
        assert_eq!(
            credited.iter().filter(|id| **id == theirs).count(),
            1,
            "{lie:?}"
        );
        assert_eq!(finalized, [ours], "{lie:?}");
        assert_eq!(cancelled, before.2, "{lie:?}");
        assert_eq!((reconciled, write_offs), (before.3, before.4), "{lie:?}");
    }
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: a settlement of
// ours that never applied, while the account's sequence moved past it with
// no transaction taking its number, as for an account deleted and made
// again, can be proven neither applied nor absent. A server that proves it
// applied, in a ledger the anchor's chain does not hold, is caught: it is
// not booked. Nor is it cancelled once the server is honest: it stays
// pending for the owner, and is counted.
#[test]
fn a_made_up_proof_of_ours_books_nothing() {
    let (mut world, state_a, _) = funded_pair();
    let account = world.account(0);
    let sequence = world.ledger.sequence(account).unwrap();
    let ours = world.settle(0, 1, 2_000_000_000, &state_a);
    world.ledger.set_sequence(account, sequence + 5);
    world.nodes[0].lagging = true;
    world.ledger.set_lie_for_all(Some(Lie::Transaction));
    world.closes(30);
    assert!(world.ledger.lies_told(Lie::Transaction) > 0);
    assert!(world.nodes[0].scheme.account.doubts(Doubt::Ledger) > 0);
    let books = &world.nodes[0].books;
    assert!(books.finalized.is_empty() && books.cancelled.is_empty());
    world.ledger.set_lie_for_all(None);
    world.wait(STALL_MS);
    world.closes(3);
    world.nodes[0].lagging = false;
    let books = &world.nodes[0].books;
    assert!(books.finalized.is_empty() && books.cancelled.is_empty());
    assert!(books.pending.contains_key(&ours));
    assert!(world.nodes[0].scheme.account.unproven().settlements > 0);
    // The next settlement takes the sequence the anchor proves, and applies.
    let next = world.settle(0, 1, 1_000_000_000, &state_a);
    world.close();
    assert_eq!(world.nodes[0].books.finalized, [next]);
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: a settlement of
// ours that never applied is cancelled only once proven absent: the
// account's sequence, proven at an anchor past its last ledger, is not
// past its own.
#[test]
fn a_settlement_is_cancelled_only_once_proven_absent() {
    let (mut world, state_a, _) = funded_pair();
    world.nodes[0].drop_submits = true;
    let id = world.settle(0, 1, 3_000_000_000, &state_a);
    world.closes(20);
    assert!(
        world.nodes[0].books.cancelled.is_empty(),
        "not yet past its last ledger"
    );
    world.closes(2);
    assert_eq!(world.nodes[0].books.cancelled, [id]);
    assert_eq!(world.nodes[0].scheme.account.unproven().settlements, 0);
}
