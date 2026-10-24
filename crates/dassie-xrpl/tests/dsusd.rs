//! The token scheme in DSUSD (`xrpl-testnet-dsusd`) against the fake
//! ledger, alone and beside `xrpl-testnet` and `xrpl-testnet-rlusd` on the
//! one account (spec/areas/xrpl.md#dsusd): its line set before it syncs,
//! settling, booking by the line, and each XRPL scheme booking only its own
//! settlements, with the books checked against the ledger after every
//! ledger. RLUSD's own tests (`tests/rlusd.rs`) cover the token scheme's
//! rules in depth; this pins that a second token runs on them unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::needless_range_loop,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};

use dassie_ledger::{LedgerId, Name};
use dassie_node_core::testing::SeededRng;
use dassie_settlement::{
    AssetEffect, Effect, ExternalAsset, LedgerInput, LedgerStatus, SettlementScheme,
};
use dassie_xrpl::account::{self, XrplAccount};
use dassie_xrpl::codec::{Amount, Memo, Payment, Signing, TokenValue, Transact};
use dassie_xrpl::fake::{FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC, Token};
use dassie_xrpl::token::UNITS_PER_MICRO;
use dassie_xrpl::{
    AccountId, DsusdScheme, RlusdScheme, UNITS_PER_DROP, Wallet, XrplScheme, dsusd, rlusd,
};

/// The XRPL schemes, by their index in a node.
const XRP: usize = 0;
const RLUSD: usize = 1;
const DSUSD: usize = 2;
const SCHEMES: [&str; 3] = [
    dassie_xrpl::scheme::SCHEME_ID,
    rlusd::SCHEME_ID,
    dsusd::SCHEME_ID,
];

/// What the node would book for a scheme, in internal units.
#[derive(Debug, Default)]
struct Books {
    /// `assets/settlement`, as a debit balance.
    settlement: i128,
    owner: i128,
    suspense: i128,
    fees: i128,
    deposits: Vec<u64>,
    peers: BTreeMap<Name, i128>,
    pending: BTreeMap<Name, (Name, u64)>,
    credited: BTreeSet<Name>,
    finalized: Vec<Name>,
}

impl Books {
    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::Deposit { amount } => {
                self.settlement += i128::from(amount);
                self.owner += i128::from(amount);
                self.deposits.push(amount);
            }
            Effect::OwnerPaid { .. } => panic!("a token scheme pays the owner nothing"),
            Effect::Withdrawal { amount } => {
                self.settlement -= i128::from(amount);
                self.owner -= i128::from(amount);
            }
            Effect::Reconcile { amount, gain, .. } => {
                let amount = if gain {
                    i128::from(amount)
                } else {
                    -i128::from(amount)
                };
                self.settlement += amount;
                self.suspense += amount;
            }
            Effect::Fee { amount, .. } => {
                self.settlement -= i128::from(amount);
                self.fees += i128::from(amount);
            }
            Effect::IncomingSettlement { peer, amount, id } => {
                assert!(self.credited.insert(id.unwrap()), "credited twice");
                self.settlement += i128::from(amount);
                *self.peers.entry(peer).or_default() += i128::from(amount);
            }
            Effect::Finalize { settlement } => {
                let (peer, amount) = self.pending.remove(&settlement).unwrap();
                self.settlement -= i128::from(amount);
                *self.peers.entry(peer).or_default() -= i128::from(amount);
                self.finalized.push(settlement);
            }
            Effect::Cancel { settlement } => {
                self.pending.remove(&settlement).unwrap();
            }
            Effect::SendMessage { .. } => {}
            Effect::PeerLoss { .. } => panic!("an XRPL scheme books no peer losses"),
        }
    }

    /// What the owner may spend, as the node tells the account.
    fn funds(&self) -> u64 {
        let pending: i128 = self.pending.values().map(|(_, a)| i128::from(*a)).sum();
        u64::try_from((self.owner + self.suspense - self.fees - pending).max(0)).unwrap()
    }
}

fn name(text: &str) -> Name {
    Name::new(text).unwrap()
}

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

/// Dollars in internal units.
const fn usd(micros: u64) -> u64 {
    micros * UNITS_PER_MICRO
}

/// A node: one account, and the XRPL schemes it runs on it, by index.
struct Node {
    name: Name,
    seed: u8,
    account: XrplAccount,
    schemes: [Option<Box<dyn SettlementScheme>>; 3],
    books: [Books; 3],
    connection: u64,
}

impl Node {
    fn scheme(&mut self, index: usize) -> &mut dyn SettlementScheme {
        self.schemes[index].as_deref_mut().unwrap()
    }

    fn status(&self, index: usize) -> LedgerStatus {
        match self.account.status() {
            LedgerStatus::Synced => self.schemes[index].as_ref().unwrap().status(),
            other => other,
        }
    }
}

struct World {
    ledger: FakeLedger,
    nodes: Vec<Node>,
    now: u64,
    next_connection: u64,
}

impl World {
    /// Nodes on a fresh fake ledger, each running the schemes `runs` names,
    /// all on the node's one account, funded by the faucet.
    fn new(runs: &[&[usize]]) -> Self {
        let mut world = Self {
            ledger: FakeLedger::new(),
            nodes: Vec::new(),
            now: 1_000,
            next_connection: 1,
        };
        for (index, schemes) in runs.iter().enumerate() {
            world.add(u8::try_from(index).unwrap() + 1, schemes);
        }
        world
    }

    fn add(&mut self, seed: u8, runs: &[usize]) {
        let node = name(&format!("node{seed}"));
        let ids: Vec<Name> = SCHEMES.iter().map(|id| name(id)).collect();
        let account = XrplAccount::new(account::FIRST, &ids, "", &[seed; 32], &[]).unwrap();
        self.ledger.fund(account.account());
        self.close();
        let make = |index: usize| -> Option<Box<dyn SettlementScheme>> {
            if !runs.contains(&index) {
                return None;
            }
            Some(match index {
                XRP => Box::new(XrplScheme::new(&account, node, None, &[]).unwrap()),
                RLUSD => Box::new(RlusdScheme::new(&account, node, None, &[]).unwrap()),
                _ => Box::new(DsusdScheme::new(&account, node, None, &[]).unwrap()),
            })
        };
        let schemes = [make(XRP), make(RLUSD), make(DSUSD)];
        self.nodes.push(Node {
            name: node,
            seed,
            account,
            schemes,
            books: Default::default(),
            connection: 0,
        });
        let index = self.nodes.len() - 1;
        self.tell(index);
        self.next_connection += 1;
        self.nodes[index].connection = self.next_connection;
        self.input(index, LedgerInput::Connected);
    }

    fn address(&self, index: usize) -> AccountId {
        self.nodes[index].account.account()
    }

    fn input(&mut self, index: usize, input: LedgerInput<'_>) {
        self.nodes[index].account.on_ledger(self.now, input);
        self.tell(index);
    }

    /// Hands every scheme on the account what it learnt, in the node's
    /// order, with the owner's funds first; books what they say, and
    /// carries out what the account asks.
    fn tell(&mut self, index: usize) {
        let now = self.now;
        let node = &mut self.nodes[index];
        for scheme in 0..3 {
            if node.schemes[scheme].is_some() {
                let funds = node.books[scheme].funds();
                node.account.owner_funds(name(SCHEMES[scheme]), funds);
            }
        }
        for scheme in 0..3 {
            let mut effects = Vec::new();
            if let Some(running) = &mut node.schemes[scheme] {
                running.on_asset(now, &mut node.account, &mut effects);
                running.check_invariants();
            }
            for effect in effects {
                node.books[scheme].apply(effect);
            }
        }
        node.account.told();
        node.account.check_invariants();
        self.carry_out(index);
    }

    fn carry_out(&mut self, index: usize) {
        let mut asked = Vec::new();
        self.nodes[index].account.take_effects(&mut asked);
        for effect in asked {
            match effect {
                AssetEffect::Send { message } => {
                    let reply = self.ledger.request(self.nodes[index].connection, &message);
                    self.input(index, LedgerInput::Message(&reply));
                }
                AssetEffect::Reconnect => {
                    let connection = self.nodes[index].connection;
                    self.ledger.disconnect(connection);
                    self.input(index, LedgerInput::Disconnected);
                    self.next_connection += 1;
                    self.nodes[index].connection = self.next_connection;
                    self.input(index, LedgerInput::Connected);
                }
                AssetEffect::Fund { .. } => panic!("the faucet funded the account"),
                AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
            }
        }
    }

    /// Closes a ledger, delivers the streams, and checks every node's books
    /// against the ledger.
    fn close(&mut self) {
        self.now += 4_000;
        for (connection, message) in self.ledger.close() {
            if let Some(index) = self.nodes.iter().position(|n| n.connection == connection) {
                self.input(index, LedgerInput::Message(&message));
            }
        }
        self.check();
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
        }
    }

    /// Every node's books match the ledger: each token scheme's the line of
    /// its own token in whole millionths, the XRP scheme's the XRP less the
    /// reserve. The ledger made and lost no XRP and no token.
    fn check(&self) {
        for node in &self.nodes {
            let address = node.account.account();
            for (index, token) in [(RLUSD, Token::RLUSD), (DSUSD, Token::DSUSD)] {
                let Some(scheme) = &node.schemes[index] else {
                    continue;
                };
                assert_eq!(node.books[index].settlement, i128::from(scheme.balance()));
                if node.status(index) == LedgerStatus::Synced {
                    let held = self
                        .ledger
                        .token_balance(address, token)
                        .map_or(0, |value| value.floor_units(6).unwrap());
                    assert_eq!(
                        i128::from(scheme.balance()),
                        held.max(0) * i128::from(UNITS_PER_MICRO),
                        "{} on {}",
                        node.name,
                        SCHEMES[index]
                    );
                }
            }
            if let Some(xrp) = &node.schemes[XRP] {
                assert_eq!(node.books[XRP].settlement, i128::from(xrp.balance()));
                if node.status(XRP) == LedgerStatus::Synced {
                    let balance = self.ledger.balance(address).unwrap();
                    let bookable = (balance * UNITS_PER_DROP).saturating_sub(xrp.reserve());
                    assert_eq!(xrp.balance(), bookable, "{}", node.name);
                }
            }
        }
        assert_eq!(self.ledger.total(), dassie_xrpl::MAX_DROPS);
        assert!(self.ledger.tokens_conserved());
    }

    /// Peers `a` and `b` on scheme `index`: `a` asks. Returns the state `a`
    /// keeps for `b`.
    fn peer(&mut self, index: usize, a: usize, b: usize) -> Vec<u8> {
        let (name_a, name_b) = (self.nodes[a].name, self.nodes[b].name);
        let mut info = Vec::new();
        self.nodes[b].scheme(index).peering_info(&mut info);
        let mut request = Vec::new();
        self.nodes[a]
            .scheme(index)
            .create_peering_request(name_b, &info, &mut request)
            .unwrap();
        let mut response = Vec::new();
        let state_at_b = self.nodes[b]
            .scheme(index)
            .accept_peering_request(name_a, &request, &mut response)
            .unwrap();
        let state_at_a = self.nodes[a]
            .scheme(index)
            .finalize_peering_request(name_b, &info, &response)
            .unwrap();
        self.nodes[b].scheme(index).on_peered(name_a, &state_at_b);
        self.nodes[a].scheme(index).on_peered(name_b, &state_at_a);
        state_at_a
    }

    /// `from` settles `amount` with `to` on scheme `index`: prepared, then
    /// executed, as the node does once its pending transfer is on disk.
    fn settle(&mut self, index: usize, from: usize, to: usize, amount: u64, state: &[u8]) -> Name {
        let peer = self.nodes[to].name;
        let node = &mut self.nodes[from];
        if node.schemes[XRP].is_some() {
            let funds = node.books[XRP].funds();
            node.account.owner_funds(name(SCHEMES[XRP]), funds);
        }
        let mut rng = SeededRng::new(1);
        let scheme = node.schemes[index].as_deref_mut().unwrap();
        let prepared = scheme
            .prepare_settlement(&mut rng, Some(&mut node.account), peer, amount, state)
            .unwrap();
        node.books[index]
            .pending
            .insert(prepared.id, (peer, prepared.amount));
        let mut effects = Vec::new();
        scheme.execute_settlement(Some(&mut node.account), prepared.id, &mut effects);
        for effect in effects {
            node.books[index].apply(effect);
        }
        self.carry_out(from);
        prepared.id
    }

    /// The fake's issuer of `token` mints `amount` of it to node `index`.
    fn mint(&mut self, index: usize, token: Token, amount: &str) {
        let to = self.address(index);
        self.ledger.issue(to, token.amount(value(amount))).unwrap();
    }
}

// spec/areas/xrpl.md#dsusd: the DSUSD scheme is the token scheme with
// DSUSD's issuer, currency, memo type and ledger; it sets its line, syncs,
// books what is minted to it as the owner's deposit, and settles whole
// millionths of DSUSD between two nodes that run it alone.
#[test]
fn two_nodes_settle_dsusd_on_the_fake_ledger() {
    let mut world = World::new(&[&[DSUSD], &[DSUSD]]);
    world.closes(3);
    for index in 0..2 {
        let node = &world.nodes[index];
        assert_eq!(node.status(DSUSD), LedgerStatus::Synced, "{}", node.name);
        let scheme = node.schemes[DSUSD].as_ref().unwrap();
        assert!(scheme.has_synced());
        assert_eq!(scheme.ledger(), LedgerId::XrplTestnetDsusdUsd);
        assert_eq!(scheme.id().as_str(), "xrpl-testnet-dsusd");
        assert_eq!(
            scheme.issuer().as_deref(),
            Some("rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M")
        );
        let line = world
            .ledger
            .line(world.address(index), dsusd::ISSUER, dsusd::CURRENCY)
            .unwrap()
            .view(world.address(index))
            .unwrap();
        assert!(line.ours.no_ripple);
        assert_eq!(line.limit, TokenValue::MAX);
        // No RLUSD line: the scheme follows its own token's alone.
        assert!(
            world
                .ledger
                .line(world.address(index), rlusd::ISSUER, rlusd::CURRENCY)
                .is_none()
        );
    }
    let state_a = world.peer(DSUSD, 0, 1);
    world.mint(0, Token::DSUSD, "50");
    world.close();
    assert_eq!(world.nodes[0].books[DSUSD].deposits, [usd(50_000_000)]);
    // Less than a millionth is not settled.
    let id = world.settle(DSUSD, 0, 1, usd(12_500_000) + 999, &state_a);
    world.close();
    let (a, b) = (&world.nodes[0].books[DSUSD], &world.nodes[1].books[DSUSD]);
    assert_eq!(a.finalized, [id]);
    assert_eq!(a.peers[&world.nodes[1].name], -i128::from(usd(12_500_000)));
    assert_eq!(b.peers[&world.nodes[0].name], i128::from(usd(12_500_000)));
    assert_eq!(b.credited.len(), 1);
    // Gas, off the books without the XRP scheme.
    assert_eq!((a.fees, b.fees), (0, 0));
    assert_eq!(
        world.ledger.token_balance(world.address(1), Token::DSUSD),
        Some(value("12.5"))
    );
    assert_eq!(
        world.ledger.token_balance(world.address(0), Token::DSUSD),
        Some(value("37.5"))
    );
}

// spec/areas/xrpl.md#the-account: XRP, RLUSD and DSUSD settle through the
// node's one account, its sequences handed out in order, each scheme
// booking only its own token's settlements and every fee booked once, as
// gas on the XRP ledger.
#[test]
fn a_node_runs_all_three_xrpl_schemes_on_one_account() {
    let all: &[usize] = &[XRP, RLUSD, DSUSD];
    let mut world = World::new(&[all, all]);
    world.closes(4);
    for node in &world.nodes {
        for index in 0..3 {
            assert_eq!(
                node.status(index),
                LedgerStatus::Synced,
                "{}",
                SCHEMES[index]
            );
        }
    }
    let holdings: Vec<String> = world.nodes[0]
        .account
        .holdings()
        .into_iter()
        .map(|holding| holding.code)
        .collect();
    assert_eq!(holdings.len(), 3);
    assert!(
        ["XRP", "RLUSD", "DSUSD"]
            .iter()
            .all(|code| holdings.contains(&(*code).to_string()))
    );
    let states: Vec<Vec<u8>> = (0..3).map(|index| world.peer(index, 0, 1)).collect();
    world.mint(0, Token::RLUSD, "40");
    world.mint(0, Token::DSUSD, "70");
    world.close();
    assert_eq!(
        world.nodes[0].books[RLUSD].owner,
        i128::from(usd(40_000_000))
    );
    assert_eq!(
        world.nodes[0].books[DSUSD].owner,
        i128::from(usd(70_000_000))
    );
    let mut ids: [Vec<Name>; 3] = Default::default();
    for round in 0..3 {
        // All three sign in one ledger: consecutive sequences.
        ids[XRP].push(world.settle(XRP, 0, 1, 1_000_000 + round, &states[XRP]));
        ids[RLUSD].push(world.settle(RLUSD, 0, 1, usd(1_000_000 + round), &states[RLUSD]));
        ids[DSUSD].push(world.settle(DSUSD, 0, 1, usd(2_000_000 + round), &states[DSUSD]));
        world.close();
    }
    world.closes(2);
    let (a, b) = (&world.nodes[0], &world.nodes[1]);
    for index in 0..3 {
        assert_eq!(a.books[index].finalized, ids[index], "{}", SCHEMES[index]);
        assert_eq!(b.books[index].credited.len(), 3, "{}", SCHEMES[index]);
    }
    let paid = |index: usize| b.books[index].peers[&a.name];
    assert_eq!(paid(RLUSD), i128::from(usd(3_000_003)));
    assert_eq!(paid(DSUSD), i128::from(usd(6_000_003)));
    assert_eq!(
        world.ledger.token_balance(world.address(1), Token::DSUSD),
        Some(value("6.000003"))
    );
    assert_eq!(
        world.ledger.token_balance(world.address(1), Token::RLUSD),
        Some(value("3.000003"))
    );
    // Every fee: two TrustSets and nine settlements, on the XRP ledger.
    let fees = |node: &Node| node.books[XRP].fees;
    assert_eq!(fees(a), i128::from(11 * FEE_BASE * UNITS_PER_DROP));
    assert_eq!(fees(b), i128::from(2 * FEE_BASE * UNITS_PER_DROP));
    assert!((RLUSD..=DSUSD).all(|index| a.books[index].fees == 0));
    // Both lines' reserves are held back from the XRP the books hold.
    let reserve = (RESERVE_BASE + 2 * RESERVE_INC) * UNITS_PER_DROP;
    assert_eq!(a.schemes[XRP].as_ref().unwrap().reserve(), reserve);
}

// spec/areas/xrpl.md#the-payment-it-signs: each scheme has its own memo
// type, so a Payment of DSUSD that carries RLUSD's memo, from the peer's
// own address, is no settlement on either: the DSUSD scheme books it as a
// deposit, and the RLUSD scheme books nothing.
#[test]
fn a_payment_with_another_schemes_memo_is_no_settlement() {
    let both: &[usize] = &[RLUSD, DSUSD];
    let mut world = World::new(&[both, both]);
    world.closes(4);
    world.peer(RLUSD, 0, 1);
    world.peer(DSUSD, 0, 1);
    world.mint(0, Token::DSUSD, "10");
    world.close();
    let wallet = Wallet::from_entropy(&[world.nodes[0].seed; 32]);
    let address = wallet.account();
    let info = serde_json::json!({ "command": "account_info", "account": address.to_address() });
    let reply: serde_json::Value =
        serde_json::from_slice(&world.ledger.request(0, info.to_string().as_bytes())).unwrap();
    let sequence = u32::try_from(
        reply["result"]["account_data"]["Sequence"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let signed = Payment {
        account: address,
        destination: world.address(1),
        destination_tag: None,
        amount: Amount::Token(Token::DSUSD.amount(value("4"))),
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(world.ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: vec![Memo {
            memo_type: rlusd::MEMO_TYPE.to_vec(),
            memo_data: world.nodes[0].name.as_str().as_bytes().to_vec(),
        }],
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    let submit =
        serde_json::json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
    world.ledger.request(0, submit.to_string().as_bytes());
    world.closes(2);
    let b = &world.nodes[1];
    assert!(b.books[DSUSD].credited.is_empty());
    assert_eq!(b.books[DSUSD].deposits, [usd(4_000_000)]);
    assert!(b.books[RLUSD].credited.is_empty());
    assert_eq!(b.books[RLUSD].settlement, 0);
}
