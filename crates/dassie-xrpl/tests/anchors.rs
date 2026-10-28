//! The account's anchors against the fake ledger, with real signatures
//! (spec/designs/xrpl-validation.md#6-the-account-on-proofs): it takes as
//! validated only a ledger a quorum of its own UNL signed, from lists it
//! fetched from their publishers' sites and keeps; it never takes an older
//! anchor for current; it tells a testnet reset by ancestry, and on mainnet
//! stops instead; and it stalls, with its reason, when it has no list, when
//! its server serves no proofs or does not answer, and connects anew when
//! an answer does not prove.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use dassie_settlement::{AssetEffect, ExternalAsset, LedgerInput, LedgerStatus};
use dassie_xrpl::account::{
    Doubt, LIST_REFRESH_MS, LIST_RETRY_MS, MAX_SETTLING_MS, MAX_UNSAVED_TRIES, STALL_MS,
    UNSAVED_RETRY_MS, XrplAccount,
};
use dassie_xrpl::anchor::{Ancestry, KeptAnchor, ProvenLists, ancestry};
use dassie_xrpl::fake::{FakeLedger, LIST_SITE, Lie, Unsaved, publisher_key};
use dassie_xrpl::network::{MAINNET, Network, TESTNET};
use dassie_xrpl::skip_list::{FLAG_INTERVAL, SkipList, flag_above, flag_key, recent_key};
use dassie_xrpl::unl::{Anchor, MAX_PENDING, ledger_seconds};
use proptest::prelude::*;
use serde_json::{Value, json};

/// Most rounds of requests and answers until the account asks nothing
/// more.
const MAX_ROUNDS: usize = 256;

/// One account against a fake ledger, as the node runs it, without
/// schemes: what it asks goes to the ledger, or to the list's site.
struct Harness {
    ledger: FakeLedger,
    account: XrplAccount,
    now: u64,
    connection: u64,
    /// Sites that do not answer.
    failing: BTreeSet<String>,
    /// Commands the server leaves unanswered.
    unanswered: BTreeSet<&'static str>,
    /// The commands the account sent, in order.
    asked: Vec<String>,
    /// Whether each `validations` request asked for `untrusted`, in order.
    untrusted: Vec<bool>,
    /// The sites it fetched, in order.
    fetched: Vec<String>,
    reconnects: usize,
    /// What the server does to its next answers to the proof methods, one
    /// each, in order; honest once none is left.
    tampers: VecDeque<Tamper>,
    /// The first answer of each proof method, which a replay gives again.
    first: BTreeMap<String, Value>,
}

/// The methods that prove, and `tx`, whose bytes build the leaves of
/// transactions' proofs: their answers a server may tamper with.
const PROOF_METHODS: [&str; 5] = ["state_proof", "tx_proof", "tx", "validations", "manifest"];

/// What a dishonest server does to an answer of a proof method.
#[derive(Clone, Copy, Debug)]
enum Tamper {
    /// Nothing.
    Keep,
    /// Changes one hex digit of one of its byte strings: by `pick` among
    /// them, at `at` within it.
    Flip { pick: usize, at: usize },
    /// Leaves an entry or a validation out.
    Drop,
    /// Says the state holds none of the objects: each branch without the
    /// leaf that holds its object.
    Absent,
    /// Answers with an error instead.
    Refuse(&'static str),
    /// Answers as it first did, for an older ledger.
    Replay,
    /// Sends one validation alone, and says it cut the answer, as a proof
    /// server that trusts other validators than the account's does.
    Cut,
}

/// Every byte string in `result` an answer carries, in a fixed order.
fn byte_strings(result: &mut Value) -> Vec<&mut String> {
    let mut out = Vec::new();
    let Some(object) = result.as_object_mut() else {
        return out;
    };
    for (key, value) in object.iter_mut() {
        match (key.as_str(), value) {
            ("ledger_header" | "manifest" | "tx" | "meta", Value::String(text)) => {
                out.push(text);
            }
            ("path", Value::Array(nodes)) => {
                out.extend(nodes.iter_mut().filter_map(|n| match n {
                    Value::String(text) => Some(text),
                    _ => None,
                }));
            }
            ("validations", Value::Array(validations)) => {
                out.extend(validations.iter_mut().filter_map(|v| match v {
                    Value::String(text) => Some(text),
                    _ => None,
                }));
            }
            ("entries", Value::Array(entries)) => {
                for entry in entries {
                    let Some(entry) = entry.as_object_mut() else {
                        continue;
                    };
                    if let Some(Value::Array(nodes)) = entry.get_mut("path") {
                        out.extend(nodes.iter_mut().filter_map(|n| match n {
                            Value::String(text) => Some(text),
                            _ => None,
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

impl Tamper {
    /// `reply`, an answer to `command`, as a dishonest server gives it;
    /// `first`, its first answer to the command.
    fn apply(self, reply: Vec<u8>, first: Option<&Value>) -> Vec<u8> {
        let mut reply: Value = serde_json::from_slice(&reply).unwrap();
        let id = reply["id"].clone();
        match self {
            Self::Keep => {}
            Self::Flip { pick, at } => {
                let mut strings = byte_strings(&mut reply["result"]);
                let count = strings.len();
                if let Some(text) = strings.get_mut(pick % count.max(1))
                    && !text.is_empty()
                {
                    let at = at % text.len();
                    let digit = text.as_bytes()[at];
                    let other = if digit == b'0' { 'F' } else { '0' };
                    text.replace_range(at..=at, &other.to_string());
                }
            }
            Self::Drop => {
                for key in ["entries", "validations", "path"] {
                    if let Some(list) = reply["result"][key].as_array_mut() {
                        list.pop();
                    }
                }
            }
            Self::Absent => {
                if let Some(entries) = reply["result"]["entries"].as_array_mut() {
                    for entry in entries {
                        let Some(path) = entry["path"].as_array_mut() else {
                            continue;
                        };
                        // "MLN\0" in hex: a state leaf.
                        if path
                            .first()
                            .and_then(Value::as_str)
                            .is_some_and(|node| node.starts_with("4D4C4E00"))
                        {
                            path.remove(0);
                        }
                    }
                }
            }
            Self::Refuse(error) => {
                reply = json!({
                    "type": "response", "id": id, "status": "error", "error": error,
                });
            }
            Self::Replay => {
                if let Some(first) = first {
                    reply = first.clone();
                    reply["id"] = id;
                }
            }
            Self::Cut => {
                if let Some(validations) = reply["result"]["validations"].as_array_mut() {
                    validations.truncate(1);
                    reply["result"]["truncated"] = true.into();
                }
            }
        }
        serde_json::to_vec(&reply).unwrap()
    }
}

/// A config trusting the fake's publisher of `network`, at `sites`.
fn config(network: &Network, sites: &[&str]) -> String {
    let seed = if *network == MAINNET {
        dassie_xrpl::fake::MAINNET_SEED
    } else {
        dassie_xrpl::fake::DEFAULT_SEED
    };
    json!({
        "server": "ws://ledger.test",
        "faucet": null,
        "publishers": [{
            "key": publisher_key(seed).to_node_public(),
            "sites": sites,
        }],
    })
    .to_string()
}

impl Harness {
    /// The account of `network` on `ledger`, from `state`, funded, its
    /// list served at [`LIST_SITE`] and at a second site that fails.
    fn new(ledger: FakeLedger, state: &[u8]) -> Self {
        let network = ledger.network();
        let config = config(network, &[LIST_SITE, "https://second.example/vl"]);
        let account = XrplAccount::new(
            network.first(),
            &[network.xrp_scheme],
            &config,
            &[7; 32],
            state,
        )
        .unwrap();
        let now = ledger.now_ms();
        let mut harness = Self {
            ledger,
            account,
            now,
            connection: 0,
            failing: BTreeSet::new(),
            unanswered: BTreeSet::new(),
            asked: Vec::new(),
            untrusted: Vec::new(),
            fetched: Vec::new(),
            reconnects: 0,
            tampers: VecDeque::new(),
            first: BTreeMap::new(),
        };
        harness.failing.insert("https://second.example/vl".into());
        let address = harness.account.account();
        harness.ledger.deposit(address, 100_000_000);
        harness.ledger.close();
        harness.now = harness.ledger.now_ms();
        harness
    }

    fn connect(&mut self) {
        self.connection += 1;
        self.input(LedgerInput::Connected);
    }

    fn disconnect(&mut self) {
        self.ledger.disconnect(self.connection);
        self.input(LedgerInput::Disconnected);
    }

    fn input(&mut self, input: LedgerInput<'_>) {
        self.account.on_ledger(self.now, input);
        self.pump();
    }

    /// Carries out what the account asks until it asks nothing more.
    fn pump(&mut self) {
        for _ in 0..MAX_ROUNDS {
            self.account.told();
            self.account.check_invariants();
            let mut effects = Vec::new();
            self.account.take_effects(&mut effects);
            if effects.is_empty() {
                return;
            }
            for effect in effects {
                self.carry_out(effect);
            }
        }
        panic!("the account kept asking");
    }

    fn carry_out(&mut self, effect: AssetEffect) {
        match effect {
            AssetEffect::Send { message } => {
                let request: Value = serde_json::from_slice(&message).unwrap();
                let command = request["command"].as_str().unwrap().to_owned();
                self.asked.push(command.clone());
                if command == "validations" {
                    self.untrusted.push(request["untrusted"] == true);
                }
                if self.unanswered.contains(command.as_str()) {
                    return;
                }
                let mut reply = self.ledger.request(self.connection, &message);
                if PROOF_METHODS.contains(&command.as_str()) {
                    let first = self
                        .first
                        .entry(command)
                        .or_insert_with(|| serde_json::from_slice(&reply).unwrap());
                    if let Some(tamper) = self.tampers.pop_front() {
                        reply = tamper.apply(reply, Some(first));
                    }
                }
                self.account
                    .on_ledger(self.now, LedgerInput::Message(&reply));
            }
            AssetEffect::Fetch { url, max_bytes } => {
                self.fetched.push(url.clone());
                let body = (!self.failing.contains(&url))
                    .then(|| self.ledger.fetch(&url))
                    .flatten()
                    .filter(|body| body.len() <= max_bytes);
                let fetched = LedgerInput::Fetched {
                    url: &url,
                    body: body.as_deref(),
                };
                self.account.on_ledger(self.now, fetched);
            }
            AssetEffect::Reconnect => {
                self.reconnects += 1;
                self.ledger.disconnect(self.connection);
                self.connection += 1;
                self.account.on_ledger(self.now, LedgerInput::Connected);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A ledger closes, at the account's clock, and the stream says so. A
    /// settlement through the account is near throughout, so it anchors on
    /// every ledger, as these tests of what an anchor proves need
    /// (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
    fn close(&mut self) {
        self.account.settling(self.now + MAX_SETTLING_MS);
        self.now += 4_000;
        let close_time = ledger_seconds(self.now);
        for (connection, message) in self.ledger.close_at(close_time) {
            if connection == self.connection {
                self.input(LedgerInput::Message(&message));
            }
        }
        self.timers();
    }

    fn closes(&mut self, count: usize) {
        for _ in 0..count {
            self.close();
        }
    }

    /// Runs the account's timers that are due.
    fn timers(&mut self) {
        if self.account.deadline().is_some_and(|at| at <= self.now) {
            self.account.on_timer(self.now);
            self.pump();
        }
    }

    /// Time passes, by `ms`, without a ledger closing.
    fn wait(&mut self, ms: u64) {
        self.now += ms;
        self.timers();
    }

    fn anchored_at(&self) -> Option<u32> {
        self.account.anchor().map(|anchor| anchor.index)
    }

    /// Whether the account's anchor is the fake's ledger of its index.
    fn anchor_is_the_ledgers(&self) -> bool {
        self.account
            .anchor()
            .is_some_and(|anchor| self.ledger.ledger_hash(anchor.index) == Some(anchor.hash))
    }
}

// spec/designs/xrpl-validation.md#1-the-rule: the account fetches its
// publisher's list, and anchors on the server's validated ledger once a
// quorum of its UNL signed it: the anchor's state proof proves the account
// too, which it never reads on the server's word. Each closed ledger moves
// the anchor, and what it takes as validated with it.
#[test]
fn an_account_anchors_on_a_ledger_its_unl_validated() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    assert_eq!(harness.fetched, [LIST_SITE]);
    assert_eq!(
        harness.asked,
        ["server_state", "subscribe", "state_proof", "validations"]
    );
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert!(harness.anchor_is_the_ledgers());
    harness.closes(3);
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert_eq!(harness.account.validated(), harness.ledger.ledger());
    assert!(harness.anchor_is_the_ledgers());
    assert_eq!(harness.reconnects, 0);
    for doubt in Doubt::ALL {
        assert_eq!(harness.account.doubts(doubt), 0, "{doubt:?}");
    }
}

// spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: without a
// list the account waits, and says why; a site that fails is followed by
// the publisher's next at once, and the first again a while later. Once a
// list comes, it anchors. Lists are kept with its state, so a restart needs
// no site.
#[test]
fn without_a_list_the_account_waits_and_fetches_again() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.failing.insert(LIST_SITE.into());
    harness.connect();
    let second = "https://second.example/vl";
    assert_eq!(harness.fetched, [LIST_SITE, second]);
    assert_eq!(harness.account.status(), LedgerStatus::Syncing);
    assert!(
        harness
            .account
            .waiting()
            .unwrap()
            .contains("validator list")
    );
    assert!(!harness.asked.iter().any(|asked| asked == "state_proof"));
    harness.closes(2);
    assert_eq!(harness.fetched.len(), 2, "a while later only");
    assert_eq!(harness.anchored_at(), None);
    harness.failing.remove(LIST_SITE);
    harness.wait(LIST_RETRY_MS);
    assert_eq!(harness.fetched.last().map(String::as_str), Some(LIST_SITE));
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
    assert_eq!(harness.account.waiting(), None);
    // Kept with its state: a restart anchors with every site down.
    let state = harness.account.state();
    harness.disconnect();
    let mut restarted = Harness {
        account: XrplAccount::new(
            TESTNET.first(),
            &[TESTNET.xrp_scheme],
            &config(&TESTNET, &[LIST_SITE]),
            &[7; 32],
            &state,
        )
        .unwrap(),
        ..harness
    };
    restarted.failing.insert(LIST_SITE.into());
    restarted.connect();
    restarted.close();
    assert_eq!(restarted.account.status(), LedgerStatus::Synced);
    assert!(restarted.anchor_is_the_ledgers());
}

// spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: an expired
// list leaves no UNL: the account takes no ledger as validated until a new
// list comes, and says why; it never falls back to the list it had.
#[test]
fn an_expired_list_stalls_the_account_until_a_new_one() {
    let mut ledger = FakeLedger::new();
    let close = ledger_seconds(ledger.now_ms());
    let expiration = u32::try_from(close).unwrap() + 60;
    ledger.publish_validator_list(expiration, None);
    let mut harness = Harness::new(ledger, &[]);
    harness.connect();
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
    harness.closes(20);
    let stalled = harness.account.validated();
    assert!(harness.account.waiting().unwrap().contains("expired"));
    harness.closes(3);
    assert_eq!(harness.account.validated(), stalled, "no anchor");
    // Fetched again a while after its sites last served none in effect.
    harness
        .ledger
        .publish_validator_list(expiration + 86_400, None);
    harness.wait(LIST_RETRY_MS);
    harness.closes(2);
    assert_eq!(harness.account.waiting(), None);
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
}

// spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: a list
// past the most the account keeps waiting is not taken, but its site
// served: it is fetched again when due, not from the next site at once.
#[test]
fn more_lists_waiting_than_are_kept_is_no_failed_fetch() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    assert_eq!(harness.fetched, [LIST_SITE]);
    let now_s = u32::try_from(ledger_seconds(harness.now)).unwrap();
    let mut days = 30;
    for _ in 0..=MAX_PENDING {
        days += 1;
        let effective = now_s + days * 86_400;
        harness
            .ledger
            .publish_validator_list(effective + 86_400, Some(effective));
        harness.wait(LIST_REFRESH_MS);
        harness.close();
    }
    assert_eq!(harness.fetched.len(), MAX_PENDING + 2);
    assert!(harness.fetched.iter().all(|url| url == LIST_SITE));
    assert_eq!(harness.account.waiting(), None);
    // Fetched again when due, not after a failure's retry.
    harness.wait(LIST_RETRY_MS);
    harness.close();
    assert_eq!(harness.fetched.len(), MAX_PENDING + 2);
}

// spec/designs/xrpl-validation.md#3-the-proof-api: a stock server serves no
// proofs: the account stalls, says so, and connects anew a while later; it
// never reads the account on the server's word.
#[test]
fn a_server_without_proofs_stalls_the_account() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.ledger.set_lie_for_all(Some(Lie::NoProofs));
    harness.connect();
    assert_eq!(harness.account.status(), LedgerStatus::Syncing);
    assert!(harness.account.waiting().unwrap().contains("no proofs"));
    assert!(!harness.asked.iter().any(|asked| asked == "account_info"));
    harness.closes(2);
    assert_eq!(harness.reconnects, 0);
    harness.ledger.set_lie_for_all(None);
    harness.wait(STALL_MS);
    assert_eq!(harness.reconnects, 1);
    harness.close();
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
}

// A server that does not answer a proof is left: the account connects anew
// and says why.
#[test]
fn a_server_that_does_not_answer_is_left() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.unanswered.insert("validations");
    harness.connect();
    assert_eq!(harness.account.status(), LedgerStatus::Syncing);
    harness.wait(20_000);
    assert_eq!(harness.reconnects, 1);
    assert!(
        harness
            .account
            .waiting()
            .unwrap()
            .contains("did not answer")
    );
    harness.unanswered.clear();
    harness.wait(20_000);
    assert_eq!(harness.reconnects, 2);
    harness.close();
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: every lie about
// the validated ledger is caught: the anchor stays the ledger's own,
// nothing is written off, the lie is counted, and the account connects
// anew, or, for an older ledger, waits.
#[test]
fn a_lying_server_moves_no_anchor() {
    for (lie, doubt) in [
        (Lie::WrongHeader, Doubt::NoQuorum),
        (Lie::FalseAbsence, Doubt::Branch),
        (Lie::OtherBranch, Doubt::Branch),
        (Lie::OtherChainSkipList, Doubt::Branch),
        (Lie::ForgedValidations, Doubt::NoQuorum),
        (Lie::ReplayedValidations, Doubt::NoQuorum),
        (Lie::WithheldValidations, Doubt::NoQuorum),
        (Lie::MadeUpReset, Doubt::NoQuorum),
        (Lie::OldAnchor, Doubt::Older),
        (Lie::OlderAnchor, Doubt::Older),
    ] {
        let mut harness = Harness::new(FakeLedger::new(), &[]);
        harness.connect();
        harness.closes(70);
        let (anchored, write_offs) = (harness.anchored_at(), harness.account.write_offs());
        harness.ledger.set_lie_for_all(Some(lie));
        harness.closes(3);
        assert!(harness.ledger.lies_told(lie) > 0, "{lie:?}");
        assert!(harness.account.doubts(doubt) > 0, "{lie:?}");
        assert!(harness.anchor_is_the_ledgers(), "{lie:?}");
        assert!(harness.anchored_at() >= anchored, "{lie:?}");
        assert_eq!(harness.account.write_offs(), write_offs, "{lie:?}");
        // A server a few ledgers behind only holds the anchor back.
        let behind = lie == Lie::OlderAnchor;
        assert!(behind || harness.account.waiting().is_some(), "{lie:?}");
        harness.ledger.set_lie_for_all(None);
        harness.wait(STALL_MS);
        harness.closes(2);
        assert_eq!(
            harness.anchored_at(),
            Some(harness.ledger.ledger()),
            "{lie:?}"
        );
        assert_eq!(harness.account.status(), LedgerStatus::Synced, "{lie:?}");
    }
}

// A transaction in the newest validated ledger: rippled proves it by
// `tx_proof` at once, but for a moment, until it saved the ledger, `tx`
// gives it without its metadata, not validated, or, for one not in its
// cache, as of a ledger it acquired, `txnNotFound`. That is no lie: the
// account asks for its bytes again a while later, and proves it then,
// with no doubt counted and no stall.
#[test]
fn bytes_a_server_has_not_saved_yet_are_asked_again() {
    for how in [Unsaved::WithoutMeta, Unsaved::NotFound] {
        not_saved_yet(how);
    }
}

fn not_saved_yet(how: Unsaved) {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    let address = harness.account.account();
    let proven = harness.account.proofs().transactions;
    harness.ledger.deposit(address, 1_000_000);
    harness.ledger.answer_unsaved(3, how);
    harness.asked.clear();
    harness.close();
    let asked = |harness: &Harness| harness.asked.iter().filter(|c| *c == "tx").count();
    assert_eq!(asked(&harness), 1);
    assert_eq!(harness.account.proofs().transactions, proven);
    for asks in 2..=3 {
        harness.wait(UNSAVED_RETRY_MS);
        assert_eq!(asked(&harness), asks, "{how:?}");
        assert_eq!(harness.account.proofs().transactions, proven, "{how:?}");
    }
    harness.wait(UNSAVED_RETRY_MS);
    assert_eq!(asked(&harness), 4, "{how:?}");
    assert_eq!(harness.account.proofs().transactions, proven + 1, "{how:?}");
    for doubt in Doubt::ALL {
        assert_eq!(harness.account.doubts(doubt), 0, "{doubt:?}");
    }
    assert_eq!((harness.reconnects, harness.account.waiting()), (0, None));
    harness.close();
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
    // A server that never gives them is taken as refusing them, a bounded
    // number of asks later: a stall, never a lie.
    harness.ledger.deposit(address, 1_000_000);
    harness.ledger.answer_unsaved(u32::MAX, how);
    harness.asked.clear();
    harness.close();
    for _ in 0..MAX_UNSAVED_TRIES {
        harness.wait(UNSAVED_RETRY_MS);
    }
    assert_eq!(asked(&harness), 1 + MAX_UNSAVED_TRIES as usize);
    assert!(harness.account.waiting().unwrap().contains("refused"));
    for doubt in Doubt::ALL {
        assert_eq!(harness.account.doubts(doubt), 0, "{doubt:?}");
    }
    harness.ledger.answer_unsaved(0, how);
    harness.wait(STALL_MS);
    harness.close();
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
}

// spec/designs/xrpl-validation.md#3-the-proof-api: a `validations` answer
// the server cut short of a quorum may be its choice of validators, not
// too few of the account's signing: the account waits, says why, and
// counts no lie; then anchors on an answer whole.
#[test]
fn a_cut_answer_short_of_a_quorum_is_no_lie() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.tampers.extend([Tamper::Keep, Tamper::Cut]);
    harness.connect();
    assert_eq!(harness.anchored_at(), None);
    assert!(harness.account.waiting().unwrap().contains("cut"));
    for doubt in Doubt::ALL {
        assert_eq!(harness.account.doubts(doubt), 0, "{doubt:?}");
    }
    harness.wait(STALL_MS);
    harness.close();
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
}

// spec/designs/xrpl-validation.md#3-the-proof-api: with the owner's
// publishers, the account's validators need not be the server's, so it
// asks for the validations of other keys from the first.
#[test]
fn with_the_owners_publishers_validations_of_other_keys_are_asked_for() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert_eq!(harness.untrusted.first(), Some(&true));
    assert!(harness.untrusted.iter().all(|asked| *asked));
}

// spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: validators
// rotate their keys before the lists show it. When too few validations
// count, the account asks the server for the newer manifests of the
// validators that did not, and counts again.
#[test]
fn rotated_keys_are_learnt_from_the_server() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    harness.ledger.rotate_validator(0);
    harness.ledger.rotate_validator(1);
    harness.close();
    assert!(harness.asked.iter().any(|asked| asked == "manifest"));
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert_eq!(harness.reconnects, 0);
}

/// A state whose anchor is `anchor`, written as the account writes it
/// (version 1): nothing signed, the anchor, and a UNL that holds nothing
/// of its publishers yet.
fn state_with_anchor(index: u32, close_time: u32, hash: [u8; 32]) -> Vec<u8> {
    let mut state = vec![1];
    state.extend_from_slice(&0u32.to_be_bytes());
    state.extend_from_slice(&0u64.to_be_bytes());
    for _ in 0..3 {
        state.extend_from_slice(&0u32.to_be_bytes());
    }
    state.push(0xff);
    state.extend_from_slice(&index.to_be_bytes());
    state.extend_from_slice(&close_time.to_be_bytes());
    state.extend_from_slice(&hash);
    state.push(0);
    state.extend_from_slice(&0u64.to_be_bytes());
    state.extend_from_slice(&0u32.to_be_bytes());
    state.extend_from_slice(&0u32.to_be_bytes());
    state.push(0);
    state
}

// spec/designs/xrpl-validation.md#7-resets-under-a-unl: a later ledger whose
// skip list does not hold the kept anchor is on another chain, however far
// the new chain has grown: on the testnet a reset, which writes the
// account off; on mainnet, which never resets, the account stops.
#[test]
fn a_later_ledger_off_the_kept_chain_is_a_reset_on_the_testnet_only() {
    for ledger in [FakeLedger::new(), FakeLedger::mainnet()] {
        let network = ledger.network();
        let close = u32::try_from(ledger_seconds(ledger.now_ms())).unwrap();
        let first = ledger.ledger();
        // An anchor of a chain gone: below this one's first ledger, before it.
        let state = state_with_anchor(first - 10, close - 400, [9; 32]);
        let mut harness = Harness::new(ledger, &state);
        harness.connect();
        if network.resets() {
            assert_eq!(harness.account.write_offs(), 1);
            assert_eq!(harness.account.status(), LedgerStatus::Synced);
            assert!(harness.anchor_is_the_ledgers());
        } else {
            assert!(matches!(
                harness.account.status(),
                LedgerStatus::Failed(why) if why.contains("another chain")
            ));
            assert_eq!(harness.account.write_offs(), 0);
        }
    }
}

// spec/designs/xrpl-validation.md#7-resets-under-a-unl: a higher ledger
// that closed earlier than the kept anchor is on no chain the anchor is on.
// On the testnet it is of an older chain, and ignored; on mainnet, which
// never resets, the UNL validated no older chain, so it is a fork, and the
// account stops.
#[test]
fn a_higher_ledger_that_closed_earlier_is_ignored_on_the_testnet_only() {
    for ledger in [FakeLedger::new(), FakeLedger::mainnet()] {
        let network = ledger.network();
        let close = u32::try_from(ledger_seconds(ledger.now_ms())).unwrap();
        let first = ledger.ledger();
        // An anchor below this chain's first ledger that closed after it.
        let state = state_with_anchor(first - 10, close + 400, [9; 32]);
        let mut harness = Harness::new(ledger, &state);
        harness.connect();
        assert_eq!(harness.account.write_offs(), 0);
        if network.resets() {
            assert!(!matches!(harness.account.status(), LedgerStatus::Failed(_)));
            assert_eq!(harness.anchored_at(), Some(first - 10));
        } else {
            assert!(matches!(
                harness.account.status(),
                LedgerStatus::Failed(why) if why.contains("another chain")
            ));
        }
    }
}

// The same chain, however far it grew while the node was away: past the
// recent ledgers' list, the flag ledgers' list and the flag ledger's own
// list prove the kept anchor, and nothing is written off.
#[test]
fn a_ledger_far_along_the_kept_chain_is_proven_on_it() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    let kept = harness.account.anchor().unwrap();
    harness.disconnect();
    for _ in 0..300 {
        harness.now += 4_000;
        harness.ledger.close_at(ledger_seconds(harness.now));
    }
    let proofs = |harness: &Harness| {
        (harness.asked.iter())
            .filter(|asked| *asked == "state_proof")
            .count()
    };
    let before = proofs(&harness);
    harness.connect();
    assert_eq!(
        proofs(&harness),
        before + 3,
        "two more than the anchor's own"
    );
    assert_eq!(harness.account.write_offs(), 0);
    assert!(harness.anchored_at().unwrap() > kept.index + 256);
    assert!(harness.anchor_is_the_ledgers());
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
}

// The validations' freshness is by the node's clock: one that runs far
// ahead sees none as fresh, and waits, saying why. That is no lie the
// account can tell: it stays on its server, and anchors on the first
// ledger whose validations are fresh again.
#[test]
fn a_clock_far_ahead_takes_no_anchor() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.now += 10 * 60_000;
    harness.connect();
    assert_eq!(harness.anchored_at(), None);
    assert_eq!(harness.account.status(), LedgerStatus::Syncing);
    assert!(harness.account.waiting().unwrap().contains("not fresh"));
    assert!(harness.account.doubts(Doubt::Older) > 0);
    assert_eq!(harness.account.doubts(Doubt::NoQuorum), 0);
    // The ledger catches up with the clock.
    harness.close();
    assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
    assert_eq!(harness.account.status(), LedgerStatus::Synced);
    assert_eq!(harness.reconnects, 0);
}

/// A tamper with an answer, drawn.
fn tamper() -> impl Strategy<Value = Tamper> {
    prop_oneof![
        2 => Just(Tamper::Keep),
        4 => (any::<usize>(), any::<usize>()).prop_map(|(pick, at)| Tamper::Flip { pick, at }),
        1 => Just(Tamper::Drop),
        1 => Just(Tamper::Absent),
        1 => prop::sample::select(&["lgrNotFound", "unknownCmd", "tooBusy"][..])
            .prop_map(Tamper::Refuse),
        1 => Just(Tamper::Replay),
        1 => Just(Tamper::Cut),
    ]
}

/// A chain of ledgers from `first` on, by its `id`, for the ancestry's
/// model: each ledger's hash names its chain and index, and its skip lists
/// are as rippled keeps them.
#[derive(Clone, Copy, Debug)]
struct ModelChain {
    id: u8,
    first: u32,
}

impl ModelChain {
    fn hash(self, index: u32) -> [u8; 32] {
        let mut hash = [0; 32];
        hash[0] = self.id;
        hash[1..5].copy_from_slice(&index.to_be_bytes());
        hash[31] = 1;
        hash
    }

    /// The chain and ledger of `hash`.
    fn of(hash: [u8; 32], chains: &[Self]) -> (Self, u32) {
        let chain = *chains.iter().find(|chain| chain.id == hash[0]).unwrap();
        (chain, u32::from_be_bytes(hash[1..5].try_into().unwrap()))
    }

    /// The list of the 256 ledgers before `index`: none for its first.
    fn recent(self, index: u32) -> Option<SkipList> {
        (index > self.first).then(|| SkipList {
            last: index - 1,
            hashes: (index.saturating_sub(256).max(self.first)..index)
                .map(|ledger| self.hash(ledger))
                .collect(),
        })
    }

    /// The list of the flag ledgers of `group` before `index`: none if it
    /// holds none yet.
    fn flags(self, group: u32, index: u32) -> Option<SkipList> {
        let flags: Vec<u32> = (0..256)
            .map(|at| (group << 16) + at * FLAG_INTERVAL)
            .filter(|&flag| flag >= self.first && flag < index)
            .collect();
        let last = *flags.last()?;
        Some(SkipList {
            last,
            hashes: flags.iter().map(|&flag| self.hash(flag)).collect(),
        })
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    // spec/designs/xrpl-validation.md#1-the-rule: nothing a server answers
    // to the proof methods, changed, cut, refused or replayed, moves the
    // anchor off the ledger's own, writes anything off or stops the
    // account, on either network; the account keeps its invariants
    // throughout, and anchors again once the server is honest.
    #[test]
    fn no_answer_moves_the_anchor_off_the_ledger(
        mainnet in any::<bool>(),
        tampers in prop::collection::vec(tamper(), 1..24),
    ) {
        let ledger = if mainnet { FakeLedger::mainnet() } else { FakeLedger::new() };
        let mut harness = Harness::new(ledger, &[]);
        harness.connect();
        harness.closes(2);
        harness.tampers.extend(tampers);
        // A tamper may stall the account for a while: enough ledgers for
        // each to be told.
        for _ in 0..256 {
            if harness.tampers.is_empty() {
                break;
            }
            harness.close();
            prop_assert!(harness.anchor_is_the_ledgers());
            prop_assert_eq!(harness.account.write_offs(), 0);
            prop_assert!(!matches!(harness.account.status(), LedgerStatus::Failed(_)));
            prop_assert!(harness.account.validated() <= harness.ledger.ledger());
        }
        prop_assert!(harness.tampers.is_empty());
        harness.wait(STALL_MS);
        harness.closes(2);
        prop_assert_eq!(harness.anchored_at(), Some(harness.ledger.ledger()));
        prop_assert_eq!(harness.account.status(), LedgerStatus::Synced);
        prop_assert_eq!(harness.account.write_offs(), 0);
    }

    // spec/designs/xrpl-validation.md#7-resets-under-a-unl: a later anchor
    // is on the kept one's chain exactly when it is, however far either
    // chain grew and wherever the other began, told by at most three skip
    // lists: the new anchor's own of recent ledgers, and two more.
    #[test]
    fn ancestry_tells_the_kept_chain_in_three_lists(
        first in 1u32..400_000,
        kept_after in 0u32..2_000,
        later in prop_oneof![1u32..300, 1u32..200_000],
        other in prop::option::of(0u32..250_000),
    ) {
        let a = ModelChain { id: 1, first };
        let kept_index = first + kept_after;
        let new_index = kept_index + later;
        // Another chain began anywhere up to the new anchor: below the
        // kept one, or after it.
        let b = other.map_or(a, |start| ModelChain {
            id: 2,
            first: new_index.saturating_sub(start).max(1),
        });
        let chains = [a, b];
        let kept = KeptAnchor::of(
            Anchor { index: kept_index, close_time: kept_index, hash: a.hash(kept_index) },
            a.recent(kept_index).as_ref(),
        );
        let new = Anchor { index: new_index, close_time: new_index, hash: b.hash(new_index) };
        let group_of = |key: [u8; 32], at: u32| {
            let candidates = kept.flag.map(|(flag, _)| flag).into_iter().chain(flag_above(kept_index));
            candidates
                .map(|flag| flag >> 16)
                .find(|&group| flag_key(group << 16) == key)
                .unwrap_or_else(|| panic!("a key of no flag list, at {at}"))
        };
        let mut lists = ProvenLists::new();
        let mut proofs = 0;
        let on_chain = loop {
            match ancestry(&kept, &new, &lists) {
                Ancestry::OnChain => break true,
                Ancestry::OffChain => break false,
                Ancestry::Lie => return Err(TestCaseError::fail("an honest chain's lists lie")),
                Ancestry::Prove { at, key } => {
                    prop_assert!(!lists.contains_key(&(at, key)), "asked twice");
                    proofs += 1;
                    prop_assert!(proofs <= 3, "more than three lists");
                    let (chain, index) = ModelChain::of(at, &chains);
                    let list = if key == recent_key() {
                        chain.recent(index)
                    } else {
                        chain.flags(group_of(key, index), index)
                    };
                    lists.insert((at, key), list);
                }
            }
        };
        prop_assert_eq!(on_chain, other.is_none());
    }
}
