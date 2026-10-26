//! The standalone verifier (spec/areas/xrpl.md#the-verifier) against the
//! fake ledger and its lying servers: what a settlement hub's validator
//! needs of its escrow, an account's root, a trust line and a signer list,
//! and a Payment into it, is proven at ledgers the fake's UNL validated, at
//! the anchor and before it; every lie a server tells it is refused, never
//! taken; and a testnet reset is told as one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::BTreeMap;

use dassie_xrpl::api::{self, SignerList};
use dassie_xrpl::codec::{
    Amount, Payment, SignerEntry, SignerListSet, Signing, TokenAmount, TokenValue, Transact,
    TrustSet,
};
use dassie_xrpl::fake::{FEE_BASE, FakeLedger, LIST_SITE, Lie, Token};
use dassie_xrpl::keys::{AccountId, Secp256k1Key};
use dassie_xrpl::records::{
    account_key, account_root, line_key, ripple_state, signer_list, signer_list_key,
};
use dassie_xrpl::unl::{Anchor, Secp256k1Signatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{
    Ask, MAX_QUERIES, Out, Query, QueryError, QueryId, STALL_MS, Verdict, Verifier, Waiting,
};
use dassie_xrpl::{Wallet, hex_upper};
use serde_json::{Value, json};

/// The connection the scene is set up over: never the verifier's.
const SETUP: u64 = 1_000;
/// The most turns one input takes to settle.
const MAX_TURNS: usize = 10_000;

fn ask(ledger: &mut FakeLedger, request: &Value) -> Result<Value, api::ApiError> {
    match api::parse(&ledger.request(SETUP, &serde_json::to_vec(request).unwrap())) {
        api::Message::Response { outcome, .. } => outcome,
        other => panic!("not a response: {other:?}"),
    }
}

fn submit(ledger: &mut FakeLedger, blob: &[u8]) {
    let result = ask(
        ledger,
        &json!({ "id": 1, "command": "submit", "tx_blob": hex_upper(blob) }),
    )
    .unwrap();
    assert_eq!(api::submit_result(&result).unwrap(), "tesSUCCESS");
}

fn close(ledger: &mut FakeLedger) {
    ledger.close();
    assert_eq!(ledger.total(), dassie_xrpl::MAX_DROPS);
}

/// What the escrow of a hub holds, and what was paid into it.
struct Scene {
    ledger: FakeLedger,
    /// The escrow, with its signer list.
    escrow: AccountId,
    /// Its signer list, as the fake keeps it.
    list: SignerList,
    /// An account with a line to the network's dollar's issuer, holding
    /// some.
    holder: AccountId,
    /// That dollar: DSUSD on the testnet, RLUSD on mainnet.
    token: Token,
    /// The holder's Payment into the escrow, and the ledger it applied in.
    payment: ([u8; 32], u32),
    /// A ledger before the escrow had its signer list.
    before_list: u32,
}

/// A hub's escrow with a signer list of three secp256k1 keys, quorum two;
/// a holder of the network's dollar on a trust line, who paid the escrow;
/// and more ledgers closed since than an anchor's validations stay fresh
/// for.
fn scene(mut ledger: FakeLedger) -> Scene {
    let token = if ledger.network().resets() {
        Token::DSUSD
    } else {
        Token::RLUSD
    };
    let master = Wallet::from_seed([1; 16]);
    let holder = Wallet::from_seed([2; 16]);
    let escrow = master.account();
    ledger.deposit(escrow, 100_000_000);
    ledger.deposit(holder.account(), 100_000_000);
    close(&mut ledger);
    let before_list = ledger.ledger();
    let mut signers: Vec<AccountId> = (11..=13)
        .map(|seed| Secp256k1Key::from_secret(&[seed; 32]).unwrap().account())
        .collect();
    signers.sort_unstable();
    let list = SignerListSet {
        account: escrow,
        signer_quorum: 2,
        signer_entries: signers
            .iter()
            .map(|account| SignerEntry {
                account: *account,
                weight: 1,
            })
            .collect(),
        fee: FEE_BASE,
        sequence: ledger.sequence(escrow).unwrap(),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(master.public_key()),
    }
    .sign(&master);
    submit(&mut ledger, &list.blob);
    let limit = token.amount(TokenValue::parse("1000").unwrap());
    let trust = TrustSet {
        account: holder.account(),
        limit,
        fee: FEE_BASE,
        sequence: ledger.sequence(holder.account()).unwrap(),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(holder.public_key()),
    }
    .sign(&holder);
    submit(&mut ledger, &trust.blob);
    close(&mut ledger);
    let held: TokenAmount = token.amount(TokenValue::parse("25").unwrap());
    ledger.issue(holder.account(), held).unwrap();
    close(&mut ledger);
    let paid = Payment {
        account: holder.account(),
        destination: escrow,
        destination_tag: Some(7),
        amount: Amount::Xrp(3_000_000),
        send_max: None,
        fee: FEE_BASE,
        sequence: ledger.sequence(holder.account()).unwrap(),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(holder.public_key()),
    }
    .sign(&holder);
    submit(&mut ledger, &paid.blob);
    close(&mut ledger);
    let payment = (paid.hash, ledger.ledger());
    for _ in 0..70 {
        close(&mut ledger);
    }
    let list = ledger.signer_list(escrow).unwrap().clone();
    assert_eq!(
        list.entries.iter().map(|e| e.account).collect::<Vec<_>>(),
        signers
    );
    Scene {
        ledger,
        escrow,
        list,
        holder: holder.account(),
        token,
        payment,
        before_list,
    }
}

/// A verifier on its own connection to the fake, as a validator's node
/// carries out what it asks.
struct Run {
    ledger: FakeLedger,
    verifier: Verifier,
    connection: u64,
    next_id: u64,
    now_ms: u64,
    verdicts: BTreeMap<QueryId, Verdict>,
    anchors: Vec<Anchor>,
    resets: Vec<Anchor>,
    reconnects: u32,
    /// The lie the next connection tells, if any.
    lie: Option<Lie>,
}

impl Run {
    /// A verifier that trusts the fake's publisher, connected.
    fn new(ledger: FakeLedger) -> Self {
        let config = UnlConfig::for_network(ledger.network().id)
            .unwrap()
            .with_override(&ledger.publishers(LIST_SITE))
            .unwrap();
        let verifier = Verifier::new(ledger.network().id, config, Box::new(Secp256k1Signatures));
        let now_ms = ledger.now_ms() + 2_000;
        let mut run = Self {
            ledger,
            verifier,
            connection: 1,
            next_id: 1,
            now_ms,
            verdicts: BTreeMap::new(),
            anchors: Vec::new(),
            resets: Vec::new(),
            reconnects: 0,
            lie: None,
        };
        run.verifier.connected(now_ms);
        run.pump();
        run
    }

    /// Carries out what the verifier asks, until it asks nothing more.
    fn pump(&mut self) {
        for _ in 0..MAX_TURNS {
            self.verifier.check_invariants();
            let outs = self.verifier.take_out();
            if outs.is_empty() {
                return;
            }
            for out in outs {
                self.carry_out(out);
            }
        }
        panic!("the verifier does not settle");
    }

    fn carry_out(&mut self, out: Out) {
        let now = self.now_ms;
        match out {
            Out::Ask(Ask::Fetch(url)) => {
                let body = self.ledger.fetch(&url);
                self.verifier.fetched(&url, body.as_deref(), now);
            }
            Out::Ask(asked) => {
                let id = self.next_id;
                self.next_id += 1;
                let request = asked.request(id).unwrap();
                let reply = self.ledger.request(self.connection, &request);
                match api::parse(&reply) {
                    api::Message::Response { id: of, outcome } => {
                        assert_eq!(of, id);
                        self.verifier.answered(&asked, outcome, now);
                    }
                    other => panic!("not a response: {other:?}"),
                }
            }
            Out::Verdict(id, verdict) => {
                assert!(self.verdicts.insert(id, verdict).is_none());
            }
            Out::Anchored(anchor) => self.anchors.push(anchor),
            Out::Reset(anchor) => self.resets.push(anchor),
            Out::Reconnect => {
                self.reconnects += 1;
                self.reconnect();
            }
        }
    }

    /// A new connection, which tells the lie set for it.
    fn reconnect(&mut self) {
        self.ledger.disconnect(self.connection);
        self.connection += 1;
        self.ledger.set_lie(self.connection, self.lie);
        self.verifier.disconnected();
        self.verifier.connected(self.now_ms);
    }

    /// Asks `query`, and carries out what it takes.
    fn query(&mut self, query: Query) -> QueryId {
        let id = self.verifier.query(query, self.now_ms).unwrap();
        self.pump();
        id
    }

    /// Closes `ledgers` ledgers, and hints the verifier to anchor.
    fn close(&mut self, ledgers: u32) {
        for _ in 0..ledgers {
            close(&mut self.ledger);
        }
        self.now_ms = self.now_ms.max(self.ledger.now_ms() + 2_000);
        self.verifier.anchor_now(self.now_ms);
        self.pump();
    }

    /// `ms` pass.
    fn wait(&mut self, ms: u64) {
        self.now_ms += ms;
        self.verifier.tick(self.now_ms);
        self.pump();
    }

    fn doubts(&self) -> u64 {
        self.verifier.doubts().values().sum()
    }
}

/// What the scene's facts are, read as a validator reads them from a
/// proven state: the escrow's XRP and signer list, and the holder's DSUSD.
fn read(scene: &Scene, verdict: &Verdict) -> (u64, SignerList, TokenValue) {
    let Verdict::State(proven) = verdict else {
        panic!("not a state: {verdict:?}");
    };
    let root = account_root(proven.object(&account_key(scene.escrow)).unwrap().unwrap()).unwrap();
    let (list, _) = signer_list(
        proven
            .object(&signer_list_key(scene.escrow))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let line_key = line_key(scene.holder, scene.token.issuer, scene.token.currency);
    let (line, _) = ripple_state(proven.object(&line_key).unwrap().unwrap()).unwrap();
    let view = line.view(scene.holder).unwrap();
    (root.balance, list, view.balance)
}

fn escrow_keys(scene: &Scene) -> Vec<[u8; 32]> {
    vec![
        account_key(scene.escrow),
        signer_list_key(scene.escrow),
        line_key(scene.holder, scene.token.issuer, scene.token.currency),
    ]
}

/// The truth, as the fake keeps it.
fn truth(scene: &Scene) -> (u64, SignerList, TokenValue) {
    let line = scene
        .ledger
        .line(scene.holder, scene.token.issuer, scene.token.currency)
        .unwrap();
    (
        scene.ledger.balance(scene.escrow).unwrap(),
        scene.list.clone(),
        line.view(scene.holder).unwrap().balance,
    )
}

// spec/areas/xrpl.md#the-verifier: an escrow's root, its signer list and a
// trust line, proven at the anchor and before it; a Payment into it, by its
// own proof in a ledger the anchor's chain holds; and what the server does
// not find proves nothing.
#[test]
fn an_escrow_s_state_and_a_payment_into_it_are_proven() {
    let scene = scene(FakeLedger::new());
    let facts = truth(&scene);
    assert_eq!(facts.2, TokenValue::parse("25").unwrap());
    let mut run = Run::new(scene.ledger.clone());
    let anchor = run.verifier.anchor().unwrap();
    assert_eq!(anchor.index, scene.ledger.ledger());
    assert_eq!(run.anchors, vec![anchor]);

    let mut keys = escrow_keys(&scene);
    let nobody = account_key(Wallet::from_seed([9; 16]).account());
    keys.push(nobody);
    let at_anchor = run.query(Query::State { keys, at: None });
    let verdict = &run.verdicts[&at_anchor];
    assert_eq!(read(&scene, verdict), facts);
    let Verdict::State(proven) = verdict else {
        panic!("not a state: {verdict:?}");
    };
    assert_eq!(proven.header.sequence, anchor.index);
    assert_eq!(proven.object(&nobody), Some(None), "proven absent");

    // Before the escrow had its signer list, through the anchor's skip
    // list: its root, and no list.
    let before = run.query(Query::State {
        keys: vec![account_key(scene.escrow), signer_list_key(scene.escrow)],
        at: Some(scene.before_list),
    });
    let Verdict::State(proven) = &run.verdicts[&before] else {
        panic!("{:?}", run.verdicts[&before]);
    };
    assert_eq!(proven.header.sequence, scene.before_list);
    let root = account_root(proven.object(&account_key(scene.escrow)).unwrap().unwrap()).unwrap();
    assert_eq!(root.balance, 100_000_000);
    assert_eq!(proven.object(&signer_list_key(scene.escrow)), Some(None));

    // The Payment, with its result and its ledger, at or after a ledger,
    // or before it.
    let (hash, ledger) = scene.payment;
    let applied = run.query(Query::Transaction { hash, from: ledger });
    let Verdict::Applied(proven) = &run.verdicts[&applied] else {
        panic!("{:?}", run.verdicts[&applied]);
    };
    assert_eq!(proven.header.sequence, ledger);
    let record = &proven.decoded.record;
    assert_eq!(record.hash, hash);
    assert!(record.succeeded());
    assert_eq!(
        (record.destination, record.destination_tag, record.delivered),
        (Some(scene.escrow), Some(7), Some(3_000_000))
    );
    let before = run.query(Query::Transaction {
        hash,
        from: ledger + 1,
    });
    assert!(matches!(run.verdicts[&before], Verdict::Before(_)));
    let unknown = run.query(Query::Transaction {
        hash: [7; 32],
        from: 0,
    });
    assert_eq!(run.verdicts[&unknown], Verdict::NotFound);

    // A ledger after the anchor waits for a later anchor.
    let later = run.query(Query::State {
        keys: vec![account_key(scene.escrow)],
        at: Some(anchor.index + 2),
    });
    assert!(!run.verdicts.contains_key(&later));
    run.close(1);
    assert!(!run.verdicts.contains_key(&later));
    run.close(1);
    let Verdict::State(proven) = &run.verdicts[&later] else {
        panic!("{:?}", run.verdicts[&later]);
    };
    assert_eq!(proven.header.sequence, anchor.index + 2);
    assert_eq!(run.verifier.anchor().unwrap().index, anchor.index + 2);

    // A ledger before the chain began is none of its; one the fake keeps
    // no state of any more is history the server does not have; and the
    // Payment is proven as far back as the skip list reaches.
    let first = run.query(Query::State {
        keys: vec![account_key(scene.escrow)],
        at: Some(dassie_xrpl::fake::FIRST_LEDGER - 1),
    });
    assert_eq!(run.verdicts[&first], Verdict::NoLedger);
    run.close(140);
    let old = run.query(Query::State {
        keys: vec![account_key(scene.escrow)],
        at: Some(scene.before_list),
    });
    assert_eq!(run.verdicts[&old], Verdict::NoHistory);
    let far = run.query(Query::Transaction { hash, from: ledger });
    assert!(matches!(run.verdicts[&far], Verdict::Applied(_)));

    assert_eq!(run.doubts(), 0);
    assert_eq!(run.reconnects, 0);
    assert_eq!(run.verifier.open_queries(), 0);
    let proofs = run.verifier.proofs();
    assert!(proofs.anchors >= 3 && proofs.transactions >= 2 && proofs.absences >= 2);
}

/// Asks what a validator asks of the scene: the escrow's state at the
/// anchor, the Payment, and a transaction the liar made up, if any.
fn ask_all(run: &mut Run, scene: &Scene, made_up: Option<[u8; 32]>) -> Vec<QueryId> {
    let mut ids = vec![
        run.query(Query::State {
            keys: escrow_keys(scene),
            at: None,
        }),
        run.query(Query::Transaction {
            hash: scene.payment.0,
            from: scene.payment.1,
        }),
    ];
    if let Some(hash) = made_up {
        ids.push(run.query(Query::Transaction { hash, from: 0 }));
    }
    ids
}

/// A transaction `liar` lists for the holder that the honest history does
/// not hold.
fn made_up(ledger: &mut FakeLedger, liar: u64, account: AccountId) -> [u8; 32] {
    let request = json!({ "id": 9, "command": "account_tx", "account": account.to_address() });
    let mut listed = |connection: u64| -> Vec<String> {
        let reply = ledger.request(connection, &serde_json::to_vec(&request).unwrap());
        let reply: Value = serde_json::from_slice(&reply).unwrap();
        (reply["result"]["transactions"].as_array().unwrap().iter())
            .map(|entry| entry["tx"]["hash"].as_str().unwrap().to_owned())
            .collect()
    };
    let honest = listed(SETUP);
    let lied = listed(liar);
    let hash = lied.iter().find(|hash| !honest.contains(hash)).unwrap();
    dassie_xrpl::from_hex(hash).unwrap().try_into().unwrap()
}

/// Whether `verdict` of query `index` of [`ask_all`] is true.
fn is_true(scene: &Scene, index: usize, verdict: &Verdict) -> bool {
    match (index, verdict) {
        (0, Verdict::State(_)) => read(scene, verdict) == truth(scene),
        (1, Verdict::Applied(proven)) => {
            proven.decoded.record.hash == scene.payment.0
                && proven.header.sequence == scene.payment.1
                && proven.decoded.record.succeeded()
        }
        // A server that finds nothing proves nothing.
        (1 | 2, Verdict::NotFound) => true,
        _ => false,
    }
}

/// The lies that reach what the verifier asks, and each is refused: the
/// verifier counts it, or waits for a new connection.
const REFUSED: [Lie; 14] = [
    Lie::Transaction,
    Lie::AlteredObject,
    Lie::FalseAbsence,
    Lie::OtherBranch,
    Lie::OldAnchor,
    Lie::OlderAnchor,
    Lie::ForgedValidations,
    Lie::ReplayedValidations,
    Lie::WithheldValidations,
    Lie::WrongHeader,
    Lie::OtherChainSkipList,
    Lie::Refusal,
    Lie::MadeUpReset,
    Lie::NoProofs,
];

/// `lie` told to a verifier anchored on an honest server: what it proves
/// while the server lies is true, and once a new connection is honest, it
/// proves all it was asked.
fn refuse(lie: Lie) {
    let scene = scene(FakeLedger::new());
    let mut run = Run::new(scene.ledger.clone());
    let honest_anchor = run.verifier.anchor().unwrap();
    run.ledger.set_lie(run.connection, Some(lie));
    run.lie = Some(lie);
    let made_up =
        (lie == Lie::Transaction).then(|| made_up(&mut run.ledger, run.connection, scene.holder));
    // A ledger closes, so that the liar has a newer one to lie about.
    run.close(1);
    let ids = ask_all(&mut run, &scene, made_up);
    if REFUSED.contains(&lie) {
        assert!(
            run.doubts() > 0 || run.verifier.waiting().is_some() || run.reconnects > 0,
            "{lie:?} refused"
        );
    }
    assert!(run.resets.is_empty(), "{lie:?}: no reset");
    assert!(!run.verifier.failed());
    assert!(
        run.verifier
            .anchor()
            .is_some_and(|anchor| anchor.index >= honest_anchor.index),
        "{lie:?}: the anchor never goes back"
    );
    for (index, id) in ids.iter().enumerate() {
        if let Some(verdict) = run.verdicts.get(id) {
            assert!(is_true(&scene, index, verdict), "{lie:?}: {verdict:?}");
        }
    }
    // The liar goes; the next connection is honest.
    run.lie = None;
    for _ in 0..4 {
        run.wait(STALL_MS);
    }
    for (index, id) in ids.iter().enumerate() {
        let verdict = (run.verdicts.get(id)).unwrap_or_else(|| panic!("{lie:?}: {index} open"));
        assert!(is_true(&scene, index, verdict), "{lie:?}: {verdict:?}");
    }
    assert!(
        run.ledger.lies_told(lie) > 0 || !REFUSED.contains(&lie),
        "{lie:?} told"
    );
}

#[test]
fn every_lie_is_refused() {
    for lie in Lie::ALL {
        refuse(lie);
    }
}

// A made-up transaction asked for before it is known to be one: the liar
// proves it in a header of its own, which no anchor's chain holds.
#[test]
fn a_made_up_transaction_is_never_applied() {
    let scene = scene(FakeLedger::new());
    let mut run = Run::new(scene.ledger.clone());
    run.ledger.set_lie(run.connection, Some(Lie::Transaction));
    let hash = made_up(&mut run.ledger, run.connection, scene.holder);
    let id = run.query(Query::Transaction { hash, from: 0 });
    assert!(!run.verdicts.contains_key(&id));
    assert_eq!(
        run.verifier
            .doubts()
            .get(&dassie_xrpl::anchor::Doubt::Ledger),
        Some(&1)
    );
    assert_eq!(run.verifier.waiting(), Some(Waiting::NotProven));
    run.wait(STALL_MS);
    assert_eq!(run.verdicts[&id], Verdict::NotFound);
}

// spec/decisions/ledger-resets.md: a chain the UNL validated that the kept
// anchor is not on is a testnet reset; what the queries ask is proven on
// the new chain.
#[test]
fn a_testnet_reset_is_told_and_proven_on() {
    let scene = scene(FakeLedger::new());
    let mut run = Run::new(scene.ledger.clone());
    let before = run.verifier.anchor().unwrap();
    run.ledger.reset();
    run.close(3);
    assert_eq!(run.resets.len(), 1);
    let after = run.verifier.anchor().unwrap();
    assert!(after.index < before.index && after.close_time > before.close_time);
    let id = run.query(Query::State {
        keys: vec![account_key(scene.escrow)],
        at: None,
    });
    let Verdict::State(proven) = &run.verdicts[&id] else {
        panic!("{:?}", run.verdicts[&id]);
    };
    assert_eq!(proven.object(&account_key(scene.escrow)), Some(None));
    // A ledger before the new chain began is none of its.
    let gone = run.query(Query::State {
        keys: vec![account_key(scene.escrow)],
        at: Some(after.index - 10),
    });
    assert_eq!(run.verdicts[&gone], Verdict::NoLedger);
    assert_eq!(run.doubts(), 0);
}

// On mainnet, which never resets, a made-up reset is refused and stops
// nothing; the escrow is proven as on the testnet.
#[test]
fn mainnet_takes_no_made_up_reset() {
    let scene = scene(FakeLedger::mainnet());
    let mut run = Run::new(scene.ledger.clone());
    run.ledger.set_lie(run.connection, Some(Lie::MadeUpReset));
    run.lie = Some(Lie::MadeUpReset);
    run.close(1);
    assert!(run.ledger.lies_told(Lie::MadeUpReset) > 0);
    assert!(run.resets.is_empty() && !run.verifier.failed());
    run.lie = None;
    run.wait(STALL_MS);
    let id = run.query(Query::State {
        keys: escrow_keys(&scene),
        at: None,
    });
    assert_eq!(read(&scene, &run.verdicts[&id]), truth(&scene));
}

// What it keeps comes back after a restart: the lists, and the anchor,
// below which no ledger is taken as current.
#[test]
fn its_state_survives_a_restart() {
    let scene = scene(FakeLedger::new());
    let run = Run::new(scene.ledger.clone());
    let state = run.verifier.state();
    let config = UnlConfig::for_network(scene.ledger.network().id)
        .unwrap()
        .with_override(&scene.ledger.publishers(LIST_SITE))
        .unwrap();
    let restored = Verifier::restore(
        scene.ledger.network().id,
        config,
        Box::new(Secp256k1Signatures),
        &state,
        run.now_ms,
    )
    .unwrap();
    restored.check_invariants();
    assert_eq!(restored.anchor(), run.verifier.anchor());
    assert!(restored.unl().unl(run.now_ms).is_ok());
    assert_eq!(restored.state(), state);
}

#[test]
fn queries_are_bounded_and_checked() {
    let scene = scene(FakeLedger::new());
    let mut run = Run::new(scene.ledger.clone());
    let key = account_key(scene.escrow);
    let state = |keys: Vec<[u8; 32]>| Query::State { keys, at: None };
    assert_eq!(
        run.verifier.query(state(vec![]), run.now_ms),
        Err(QueryError::Keys)
    );
    assert_eq!(
        run.verifier.query(state(vec![key, key]), run.now_ms),
        Err(QueryError::Keys)
    );
    // Asked of a ledger after the anchor, so that they stay open.
    let later = |n: u32| Query::State {
        keys: vec![key],
        at: Some(scene.ledger.ledger() + 100 + n),
    };
    for n in 0..u32::try_from(MAX_QUERIES).unwrap() {
        run.verifier.query(later(n), run.now_ms).unwrap();
    }
    assert_eq!(
        run.verifier.query(later(0), run.now_ms),
        Err(QueryError::Full)
    );
    run.pump();
    assert_eq!(run.verifier.open_queries(), MAX_QUERIES);
}

// Two chains the same UNL validated, from one ledger on: a later anchor off
// the kept one's chain, as its skip list tells, is a reset on the testnet,
// and stops the verifier on mainnet, which never resets.
#[test]
fn a_later_anchor_off_the_kept_chain_is_a_reset_or_a_fork() {
    for testnet in [true, false] {
        let fake = if testnet {
            FakeLedger::new()
        } else {
            FakeLedger::mainnet()
        };
        let scene = scene(fake);
        let mut other = scene.ledger.clone();
        let mut run = Run::new(scene.ledger.clone());
        run.close(2);
        let kept = run.verifier.anchor().unwrap();
        other.deposit(Wallet::from_seed([5; 16]).account(), 1_000_000);
        for _ in 0..5 {
            close(&mut other);
        }
        run.ledger = other;
        run.now_ms = run.now_ms.max(run.ledger.now_ms() + 2_000);
        run.reconnect();
        run.pump();
        let id = run.query(Query::State {
            keys: vec![account_key(scene.escrow)],
            at: None,
        });
        if testnet {
            assert_eq!(run.resets.len(), 1);
            let anchor = run.verifier.anchor().unwrap();
            assert!(anchor.index > kept.index && anchor.close_time > kept.close_time);
            assert!(matches!(run.verdicts[&id], Verdict::State(_)));
        } else {
            assert!(run.verifier.failed());
            assert!(run.resets.is_empty());
            assert_eq!(run.verifier.anchor(), Some(kept));
            assert!(!run.verdicts.contains_key(&id));
        }
        assert_eq!(run.doubts(), 0);
    }
}
