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

use std::collections::BTreeSet;

use dassie_settlement::{AssetEffect, ExternalAsset, LedgerInput, LedgerStatus};
use dassie_xrpl::account::{Doubt, LIST_RETRY_MS, STALL_MS, XrplAccount};
use dassie_xrpl::fake::{FakeLedger, LIST_SITE, Lie, publisher_key};
use dassie_xrpl::network::{MAINNET, Network, TESTNET};
use dassie_xrpl::unl::ledger_seconds;
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
    /// The sites it fetched, in order.
    fetched: Vec<String>,
    reconnects: usize,
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
        "floor": dassie_xrpl::fake::UNL_FLOOR,
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
            fetched: Vec::new(),
            reconnects: 0,
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
                if self.unanswered.contains(command.as_str()) {
                    return;
                }
                let reply = self.ledger.request(self.connection, &message);
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

    /// A ledger closes, at the account's clock, and the stream says so.
    fn close(&mut self) {
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
// publisher's list, anchors on the server's validated ledger once a quorum
// of its UNL signed it, and only then reads the account; each closed ledger
// moves the anchor, and what it takes as validated with it.
#[test]
fn an_account_anchors_on_a_ledger_its_unl_validated() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.connect();
    assert_eq!(harness.fetched, [LIST_SITE]);
    assert_eq!(
        harness.asked,
        [
            "server_state",
            "subscribe",
            "state_proof",
            "validations",
            "account_info"
        ]
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

/// A state whose anchor is `anchor`, written as the account writes it.
fn state_with_anchor(index: u32, close_time: u32, hash: [u8; 32]) -> Vec<u8> {
    json!({
        "sequence": 0, "in_flight": [], "never_sent": [], "parked": [], "write_offs": 0,
        "anchor": {
            "index": index, "close_time": close_time,
            "hash": dassie_xrpl::hex_upper(&hash), "flag": null,
        },
    })
    .to_string()
    .into_bytes()
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

// spec/designs/xrpl-validation.md#10-configuration-and-state: a state
// written before anchors kept its newest ledger without a hash. The first
// anchor replaces it by the chain rule: below it and closed later is a
// reset on the testnet, and on mainnet replaces it, since only the old
// hint can have been wrong.
#[test]
fn an_older_state_s_newest_ledger_is_a_hint() {
    for ledger in [FakeLedger::new(), FakeLedger::mainnet()] {
        let resets = ledger.network().resets();
        let close = ledger_seconds(ledger.now_ms());
        let state = json!({
            "sequence": 0, "in_flight": [], "never_sent": [], "write_offs": 0,
            "newest": [ledger.ledger() + 1_000, close - 400],
        })
        .to_string();
        let mut harness = Harness::new(ledger, state.as_bytes());
        harness.connect();
        assert_eq!(harness.account.write_offs(), u64::from(resets));
        assert_eq!(harness.account.status(), LedgerStatus::Synced);
        assert!(harness.anchor_is_the_ledgers());
        let written: Value = serde_json::from_slice(&harness.account.state()).unwrap();
        assert_eq!(written["newest"], Value::Null);
        assert!(written["anchor"].is_object());
    }
}

// The validations' freshness is by the node's clock: one that runs far
// ahead sees none as fresh, and waits, saying why.
#[test]
fn a_clock_far_ahead_takes_no_anchor() {
    let mut harness = Harness::new(FakeLedger::new(), &[]);
    harness.now += 10 * 60_000;
    harness.connect();
    assert_eq!(harness.anchored_at(), None);
    assert_eq!(harness.account.status(), LedgerStatus::Syncing);
    assert!(harness.account.doubts(Doubt::NoQuorum) > 0);
}
