//! Each network's dollar token scheme against the fake ledger, alone and
//! beside the network's XRP scheme on the one account
//! (spec/areas/xrpl.md#the-token-scheme): `xrpl-testnet-dsusd` on the
//! testnet, and `xrpl-rlusd` on mainnet, real money in the live realm
//! (spec/areas/xrpl.md#rlusd), owner books that peer with no one. Its line
//! set before it syncs, the owner's payments, booking by the line, and each
//! scheme booking only its own holding, with the books checked against the
//! ledger after every ledger. The token
//! scheme's rules in depth are in `tests/token.rs`, in DSUSD; this pins that
//! each network's token runs on them unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{
    AssetEffect, Effect, ExternalAsset, LedgerInput, LedgerStatus, OwnerPayment, SettlementScheme,
};
use dassie_xrpl::account::{MAX_SETTLING_MS, XrplAccount};
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::{Amount, Memo, Payment, Signing, TokenValue, Transact};
use dassie_xrpl::fake::{FAUCET_DROPS, FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC};
use dassie_xrpl::network::{MAINNET, Network, TESTNET};
use dassie_xrpl::token::UNITS_PER_MICRO;
use dassie_xrpl::{
    AccountId, DsusdScheme, RlusdScheme, UNITS_PER_DROP, Wallet, XrplMainnetScheme, XrplScheme,
    dsusd, rlusd, scheme,
};

/// A network's XRPL schemes, by their index in a node: its XRP, and its
/// dollar token.
const XRP: usize = 0;
const TOKEN: usize = 1;

/// A network, and the schemes a node runs on it.
#[derive(Clone, Copy)]
struct Net {
    network: &'static Network,
    /// The schemes' IDs, by index.
    ids: [&'static str; 2],
    /// Its dollar token, as the fake issues it.
    token: IssuedAsset,
    /// Its dollar token, as the scheme books it.
    settles_in: dassie_xrpl::Token,
}

/// The testnet: XRP, and DSUSD, its dollar.
const TESTNET_NET: Net = Net {
    network: &TESTNET,
    ids: [scheme::SCHEME_ID, dsusd::SCHEME_ID],
    token: IssuedAsset::DSUSD,
    settles_in: dsusd::TOKEN,
};

/// Mainnet: XRP, and RLUSD, its dollar.
const MAINNET_NET: Net = Net {
    network: &MAINNET,
    ids: [scheme::MAINNET_SCHEME_ID, rlusd::SCHEME_ID],
    token: IssuedAsset::RLUSD,
    settles_in: rlusd::TOKEN,
};

/// What the node would book for a scheme, in internal units.
#[derive(Debug, Default)]
struct Books {
    /// `assets/settlement`, as a debit balance.
    settlement: i128,
    owner: i128,
    suspense: i128,
    fees: i128,
    deposits: Vec<u64>,
    /// The owner's payments held against the owner's funds while they are
    /// on their way, as the node holds them, by ID.
    owner_holds: BTreeMap<Name, u64>,
    /// The owner's payments no longer on their way, in order, and whether
    /// each applied.
    paid: Vec<(Name, bool)>,
}

impl Books {
    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::Deposit { amount } => {
                self.settlement += i128::from(amount);
                self.owner += i128::from(amount);
                self.deposits.push(amount);
            }
            Effect::OwnerPaid { id, applied } => {
                let amount = self.owner_holds.remove(&id).unwrap();
                self.paid.push((id, applied));
                if applied {
                    self.settlement -= i128::from(amount);
                    self.owner -= i128::from(amount);
                }
            }
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
            Effect::IncomingSettlement { .. } | Effect::Finalize { .. } | Effect::Cancel { .. } => {
                panic!("an owner book books no settlements")
            }
            Effect::PeerLoss { .. } => panic!("an XRPL scheme books no peer losses"),
        }
    }

    /// What the owner may spend, as the node tells the account.
    fn funds(&self) -> u64 {
        let pending: i128 = self.owner_holds.values().map(|a| i128::from(*a)).sum();
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
    schemes: [Option<Box<dyn SettlementScheme>>; 2],
    books: [Books; 2],
    connection: u64,
}

impl Node {
    fn status(&self, index: usize) -> LedgerStatus {
        match self.account.status() {
            LedgerStatus::Synced => self.schemes[index].as_ref().unwrap().status(),
            other => other,
        }
    }
}

struct World {
    net: Net,
    ledger: FakeLedger,
    nodes: Vec<Node>,
    now: u64,
    next_connection: u64,
}

impl World {
    /// Nodes on a fresh fake ledger of `net`, each running the schemes
    /// `runs` names, all on the node's one account: funded by the faucet on
    /// the testnet, and by its owner's deposit of as much on mainnet, which
    /// has none.
    fn new(net: Net, runs: &[&[usize]]) -> Self {
        let ledger = if net.network.faucet.is_some() {
            FakeLedger::new()
        } else {
            FakeLedger::mainnet()
        };
        let mut world = Self {
            net,
            ledger,
            nodes: Vec::new(),
            now: dassie_xrpl::fake::FIRST_CLOSE_MS,
            next_connection: 1,
        };
        for (index, schemes) in runs.iter().enumerate() {
            world.add(u8::try_from(index).unwrap() + 1, schemes);
        }
        world
    }

    fn add(&mut self, seed: u8, runs: &[usize]) {
        let node = name(&format!("node{seed}"));
        let ids: Vec<Name> = self.net.ids.iter().map(|id| name(id)).collect();
        let first = self.net.network.first();
        // It trusts the fake ledger's validators.
        let config = dassie_xrpl::fake::with_publishers("", self.net.network);
        let account = XrplAccount::new(first, &ids, &config, &[seed; 32], &[]).unwrap();
        if self.net.network.faucet.is_some() {
            self.ledger.fund(account.account());
        } else {
            self.ledger.deposit(account.account(), FAUCET_DROPS);
        }
        self.close();
        let mainnet = self.net.network == &MAINNET;
        let make = |index: usize| -> Option<Box<dyn SettlementScheme>> {
            if !runs.contains(&index) {
                return None;
            }
            Some(match (index, mainnet) {
                (XRP, false) => Box::new(XrplScheme::new(&account, None, &[]).unwrap()),
                (XRP, true) => Box::new(XrplMainnetScheme::new(&account, None, &[]).unwrap()),
                (_, false) => Box::new(DsusdScheme::new(&account, None, &[]).unwrap()),
                (_, true) => Box::new(RlusdScheme::new(&account, None, &[]).unwrap()),
            })
        };
        let schemes = [make(XRP), make(TOKEN)];
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
        // The node's clock reads the ledger's validations as fresh.
        self.now = self.now.max(self.ledger.now_ms());
        self.nodes[index].account.on_ledger(self.now, input);
        self.tell(index);
    }

    /// Hands every scheme on the account what it learnt, in the node's
    /// order, with the owner's funds first; books what they say, and
    /// carries out what the account asks.
    fn tell(&mut self, index: usize) {
        let now = self.now;
        let node = &mut self.nodes[index];
        for scheme in [XRP, TOKEN] {
            if node.schemes[scheme].is_some() {
                let funds = node.books[scheme].funds();
                node.account.owner_funds(name(self.net.ids[scheme]), funds);
            }
        }
        for scheme in [XRP, TOKEN] {
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
                AssetEffect::Fetch { url, .. } => {
                    let body = self.ledger.fetch(&url);
                    let fetched = LedgerInput::Fetched {
                        url: &url,
                        body: body.as_deref(),
                    };
                    self.input(index, fetched);
                }
                AssetEffect::SendTo { .. } => panic!("an XRPL account names no endpoints"),
            }
        }
    }

    /// Closes a ledger, delivers the streams, and checks every node's books
    /// against the ledger. A settlement through every account is near
    /// throughout, so that each anchors on every ledger, as these tests of
    /// what is booked need (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
    fn close(&mut self) {
        for index in 0..self.nodes.len() {
            self.nodes[index]
                .account
                .settling(self.now + MAX_SETTLING_MS);
            self.tell(index);
        }
        self.now += 4_000;
        let close_time = dassie_xrpl::unl::ledger_seconds(self.now);
        for (connection, message) in self.ledger.close_at(close_time) {
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

    /// Every node's books match the ledger: the token scheme's the line of
    /// its own token in whole millionths, the XRP scheme's the XRP less the
    /// reserve. The ledger made and lost no XRP and no token.
    fn check(&self) {
        for node in &self.nodes {
            let address = node.account.account();
            if let Some(scheme) = &node.schemes[TOKEN] {
                assert_eq!(node.books[TOKEN].settlement, i128::from(scheme.balance()));
                if node.status(TOKEN) == LedgerStatus::Synced {
                    let held = self
                        .ledger
                        .token_balance(address, self.net.token)
                        .map_or(0, |value| value.floor_units(6).unwrap());
                    assert_eq!(
                        i128::from(scheme.balance()),
                        held.max(0) * i128::from(UNITS_PER_MICRO),
                        "{}",
                        node.name
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

    /// The owner of node `from` pays `amount` of the holding of scheme
    /// `index` to node `to`'s account, after a settlement check, which
    /// reads the issuer, held against the owner's funds on the books as the
    /// node holds it. Returns the payment's ID.
    fn pay(&mut self, index: usize, from: usize, to: usize, amount: u64) -> Name {
        let destination = self.address(to).to_address();
        let now = self.now;
        let node = &mut self.nodes[from];
        if let Some(token) = &mut node.schemes[TOKEN] {
            token.on_check(now, &mut node.account);
        }
        self.carry_out(from);
        let node = &mut self.nodes[from];
        if node.schemes[XRP].is_some() {
            let funds = node.books[XRP].funds();
            node.account.owner_funds(name(self.net.ids[XRP]), funds);
        }
        let payment = OwnerPayment {
            destination,
            tag: None,
            memo: None,
            amount,
        };
        let scheme = node.schemes[index].as_deref_mut().unwrap();
        let id = scheme.pay_owner(Some(&mut node.account), &payment).unwrap();
        let unit = scheme.unit();
        node.books[index]
            .owner_holds
            .insert(id, amount / unit * unit);
        self.carry_out(from);
        id
    }

    /// The fake's issuer of the network's dollar mints `amount` of it to
    /// node `index`.
    fn mint(&mut self, index: usize, amount: &str) {
        let to = self.address(index);
        let token = self.net.token;
        self.ledger.issue(to, token.amount(value(amount))).unwrap();
    }
}

// spec/areas/xrpl.md#rlusd, spec/areas/xrpl.md#dsusd: each network's token
// scheme is the token scheme with its token's issuer, currency and ledger;
// it sets its line, syncs, books what is minted to it as the owner's
// deposit, and pays the owner's payments in whole millionths, which the
// payee books as its deposit. A node follows its own network's dollar
// alone: no line to another token's issuer, test RLUSD's included.
#[test]
fn two_owners_pay_each_networks_dollar() {
    for (net, ledger, issuer) in [
        (
            TESTNET_NET,
            LedgerId::XrplTestnetDsusdUsd,
            "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M",
        ),
        (
            MAINNET_NET,
            LedgerId::XrplRlusdUsd,
            "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De",
        ),
    ] {
        let mut world = World::new(net, &[&[TOKEN], &[TOKEN]]);
        world.closes(3);
        for index in 0..2 {
            let node = &world.nodes[index];
            assert_eq!(node.status(TOKEN), LedgerStatus::Synced, "{}", node.name);
            let scheme = node.schemes[TOKEN].as_ref().unwrap();
            assert!(scheme.has_synced());
            assert!(!scheme.peers());
            assert_eq!(scheme.ledger(), ledger);
            assert_eq!(scheme.id().as_str(), net.ids[TOKEN]);
            assert_eq!(scheme.issuer().as_deref(), Some(issuer));
            assert_eq!(scheme.asset(), Some(net.network.first()));
            let address = world.address(index);
            let line = world
                .ledger
                .line(address, net.settles_in.issuer, net.settles_in.currency)
                .unwrap()
                .view(address)
                .unwrap();
            assert!(line.ours.no_ripple);
            assert_eq!(line.limit, TokenValue::MAX);
            for other in [rlusd::TOKEN, rlusd::TESTNET_TOKEN, dsusd::TOKEN] {
                if other.issuer != net.settles_in.issuer {
                    let line = world.ledger.line(address, other.issuer, other.currency);
                    assert!(line.is_none(), "{}", other.scheme.as_str());
                }
            }
        }
        world.mint(0, "50");
        world.close();
        assert_eq!(world.nodes[0].books[TOKEN].deposits, [usd(50_000_000)]);
        let id = world.pay(TOKEN, 0, 1, usd(12_500_000));
        world.close();
        let (a, b) = (&world.nodes[0].books[TOKEN], &world.nodes[1].books[TOKEN]);
        assert_eq!(a.paid, [(id, true)]);
        assert_eq!(a.owner, i128::from(usd(37_500_000)));
        assert_eq!(b.deposits, [usd(12_500_000)]);
        // Gas, off the books without the XRP scheme.
        assert_eq!((a.fees, b.fees), (0, 0));
        assert_eq!(
            world.ledger.token_balance(world.address(1), net.token),
            Some(value("12.5"))
        );
        assert_eq!(
            world.ledger.token_balance(world.address(0), net.token),
            Some(value("37.5"))
        );
    }
}

// spec/areas/xrpl.md#the-account: XRP and the network's dollar are booked
// on the node's one account, on either network, its sequences handed out in
// order, each scheme booking only its own holding and every fee booked
// once, as gas on the XRP ledger.
#[test]
fn a_node_runs_xrp_and_its_networks_dollar_on_one_account() {
    for net in [TESTNET_NET, MAINNET_NET] {
        let both: &[usize] = &[XRP, TOKEN];
        let mut world = World::new(net, &[both, both]);
        world.closes(4);
        for node in &world.nodes {
            for index in [XRP, TOKEN] {
                let status = node.status(index);
                assert_eq!(status, LedgerStatus::Synced, "{}", net.ids[index]);
            }
        }
        let mut holdings: Vec<String> = world.nodes[0]
            .account
            .holdings()
            .into_iter()
            .map(|holding| holding.code)
            .collect();
        holdings.sort_unstable();
        let mut codes = ["XRP".to_string(), net.token.currency.code()];
        codes.sort_unstable();
        assert_eq!(holdings, codes);
        world.mint(0, "40");
        world.close();
        let owner = world.nodes[0].books[TOKEN].owner;
        assert_eq!(owner, i128::from(usd(40_000_000)));
        let mut ids: [Vec<Name>; 2] = Default::default();
        for round in 0..3 {
            // Both sign in one ledger: consecutive sequences.
            ids[XRP].push(world.pay(XRP, 0, 1, (1_000 + round) * UNITS_PER_DROP));
            let amount = usd(1_000_000 + round);
            ids[TOKEN].push(world.pay(TOKEN, 0, 1, amount));
            world.close();
        }
        world.closes(2);
        let (a, b) = (&world.nodes[0], &world.nodes[1]);
        for index in [XRP, TOKEN] {
            let paid: Vec<(Name, bool)> = ids[index].iter().map(|id| (*id, true)).collect();
            assert_eq!(a.books[index].paid, paid, "{}", net.ids[index]);
        }
        assert_eq!(b.books[TOKEN].owner, i128::from(usd(3_000_003)));
        assert_eq!(
            world.ledger.token_balance(world.address(1), net.token),
            Some(value("3.000003"))
        );
        // Every fee: the TrustSet and six payments, on the XRP ledger.
        let fees = |node: &Node| node.books[XRP].fees;
        assert_eq!(fees(a), i128::from(7 * FEE_BASE * UNITS_PER_DROP));
        assert_eq!(fees(b), i128::from(FEE_BASE * UNITS_PER_DROP));
        assert_eq!(a.books[TOKEN].fees, 0);
        // The line's reserve is held back from the XRP the books hold.
        let reserve = (RESERVE_BASE + RESERVE_INC) * UNITS_PER_DROP;
        assert_eq!(a.schemes[XRP].as_ref().unwrap().reserve(), reserve);
    }
}

// spec/areas/xrpl.md#following-the-account: a Payment of RLUSD on mainnet
// is booked by what it did to the line, whatever memo it carries: one with
// the memo a settlement of any XRPL scheme carried while they peered,
// naming the payer, is a deposit like any other.
#[test]
fn a_payment_with_a_settlement_memo_is_a_deposit() {
    let mut world = World::new(MAINNET_NET, &[&[TOKEN], &[TOKEN]]);
    world.closes(4);
    world.mint(0, "10");
    world.close();
    let wallet = Wallet::from_entropy(&[world.nodes[0].seed; 32]);
    let address = wallet.account();
    let memo_types = [
        "xrpl-rlusd",
        "xrpl-testnet-rlusd",
        "xrpl-testnet-dsusd",
        "xrpl-testnet-xrp",
        "xrpl-xrp",
    ]
    .map(|id| format!("https://dassie.land/settlement/{id}/v1"));
    for memo_type in &memo_types {
        let info =
            serde_json::json!({ "command": "account_info", "account": address.to_address() });
        let reply: serde_json::Value =
            serde_json::from_slice(&world.ledger.request(0, info.to_string().as_bytes())).unwrap();
        let sequence = reply["result"]["account_data"]["Sequence"]
            .as_u64()
            .unwrap();
        let signed = Payment {
            account: address,
            destination: world.address(1),
            destination_tag: None,
            amount: Amount::Token(IssuedAsset::RLUSD.amount(value("1"))),
            send_max: None,
            fee: FEE_BASE,
            sequence: u32::try_from(sequence).unwrap(),
            last_ledger_sequence: Some(world.ledger.ledger() + 10),
            flags: 0,
            network_id: None,
            memos: vec![Memo {
                memo_type: memo_type.as_bytes().to_vec(),
                memo_data: world.nodes[0].name.as_str().as_bytes().to_vec(),
            }],
            signing: Signing::by(wallet.public_key()),
        }
        .sign(&wallet);
        let blob = dassie_xrpl::hex_upper(&signed.blob);
        let submit = serde_json::json!({ "command": "submit", "tx_blob": blob });
        world.ledger.request(0, submit.to_string().as_bytes());
        world.closes(2);
    }
    let b = &world.nodes[1];
    assert_eq!(b.books[TOKEN].deposits, [usd(1_000_000); 5]);
    assert_eq!(
        world
            .ledger
            .token_balance(world.address(1), IssuedAsset::RLUSD),
        Some(value("5"))
    );
}
