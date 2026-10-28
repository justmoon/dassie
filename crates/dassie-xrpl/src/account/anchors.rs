//! The account's validator lists and anchors
//! (spec/designs/xrpl-validation.md#6-the-account-on-proofs): the lists
//! fetched from their publishers' sites and kept, and the rounds that
//! anchor the account on a ledger a quorum of its UNL validated, which
//! alone move what it takes as validated, and tell a testnet reset by
//! ancestry.
//!
//! **A round** is a state proof at the server's validated ledger of its
//! list of recent ledgers and of everything the account follows (its root,
//! the lines its schemes follow, their issuers' roots and the fee
//! settings), that ledger's validations, newer
//! manifests of the validators that did not count if they fall short, and,
//! for a ledger more than 256 past the kept anchor, at most two more state
//! proofs of skip lists. What the anchor proves, the account takes as it is
//! there ([`super::history`]).
//!
//! **When a round starts** (spec/designs/xrpl-validation.md#6-the-account-on-proofs):
//! as the account connects, and then on every closed ledger's hint while
//! the account is **hot**, and on demand otherwise, never while one is in
//! flight or the history is walked. It is hot until it is synced, while a
//! settlement through it is expected or about to be made
//! ([`ExternalAsset::settling`](dassie_settlement::ExternalAsset::settling),
//! and for a while after a payment asked from a stale anchor), and while
//! anything waits on a proof: a transaction of ours in flight until it is
//! proven applied or absent, a transaction being proven, its bytes asked
//! again, a submission. Otherwise a round is **wanted**, and starts at once
//! or on the next closed ledger: when a hint names a transaction of the
//! account past the anchor, a closed ledger below it, or another reserve
//! than its own; when a payment needs a fresher anchor than the newest
//! ([`FRESH_ANCHOR_MS`]); when a pool or a balance is read; when the
//! faucet answered; and on the heartbeat ([`HEARTBEAT_MS`]). So an idle
//! account anchors a few times an hour, while a settlement is credited in
//! as many ledgers as when every ledger was anchored, whether or not the
//! server tells of it.
//!
//! **What a server can do.** An answer that does not prove is a lie: it is
//! counted by kind, and the account connects anew after a while
//! ([`STALL_MS`]), as after a refusal or from a stock server that serves
//! no proofs; meanwhile it waits, with its reason, as it does for a UNL it
//! has no lists for. An older anchor than the one kept is never current:
//! the account waits for a newer one.

use alloc::vec::Vec;

use dassie_settlement::AssetEffect;
use serde_json::Value;

use super::{CHAIN_FORKED, CHAIN_WENT_BACK, Phase, Request, XrplAccount};
pub use crate::anchor::Doubt;
use crate::anchor::{
    Ancestry, Judged, KeptAnchor, ProvenLists, ProvenState, ancestry, prove_state,
};
use crate::api::{self, ApiError, Validations};
use crate::skip_list::{SkipList, recent_key};
use crate::unl::{Anchor, AnchorError, Unavailable, UnlError};
use crate::validator_list::{MAX_LIST_BYTES, UnlConfig};

pub use crate::lists::{FETCH_TIMEOUT_MS, LIST_REFRESH_MS, LIST_RETRY_MS};
// How long the account waits after a server refused a proof, told a lie
// or kept too short a history, and how many validators a round asks for
// newer manifests: as the standalone verifier.
pub use crate::verifier::{MAX_MANIFEST_ASKS, STALL_MS};

/// How long an account goes without anchoring while nothing waits on its
/// ledger. A server that withholds the hint of a transaction, from the
/// stream and from the history, hides a deposit nobody expected for at
/// most this long; anything the account expects keeps it anchoring on
/// every ledger, and a hint starts a round at once. A few minutes keeps an
/// idle account's rounds, each a state proof, a ledger's validations and
/// their signature checks, to a few an hour against one every few seconds.
pub const HEARTBEAT_MS: u64 = 180_000;
/// How long ago, by the node's clock, the account may have taken its
/// anchor to sign from it, or to read a pool or a balance at it. A
/// transaction's last ledger is the anchor's index plus a window of
/// ledgers, so the anchor must be well within that window's time for the
/// last ledger never to be past already; and a pool's reading counts for
/// the peg guard only while its anchor is fresh. A few ledgers' time: an
/// account that anchors on every ledger always has one this fresh. It
/// counts from when the anchor was taken, the server's newest validated
/// ledger with fresh validations then, not from its close time, so that a
/// clock that runs ahead does not keep the account from signing.
pub const FRESH_ANCHOR_MS: u64 = 20_000;
/// How long a payment refused for a stale anchor, or the owner's ask of
/// the faucet, keeps the account anchoring on every ledger: long enough
/// for whoever asked to ask again, from the fresh anchor the round it
/// started takes, or for what the faucet sends to arrive.
pub const PAYING_MS: u64 = 60_000;
/// The longest one telling that a settlement is near keeps the account
/// anchoring on every ledger
/// ([`ExternalAsset::settling`](dassie_settlement::ExternalAsset::settling)):
/// one that is still near is told again.
pub const MAX_SETTLING_MS: u64 = 600_000;

// Why the account waits, as the owner reads it.
pub(super) const NO_LIST: &str = "waiting for validator lists from enough of the publishers";
pub(super) const LIST_EXPIRED: &str =
    "a publisher's validator list expired, and no newer one came yet";
pub(super) const PUBLISHER_REVOKED: &str = "a validator list publisher revoked its key";
pub(super) const NO_VALIDATORS: &str = "no validator is on enough of the validator lists";
pub(super) const UNL_BLOCKED: &str =
    "the validator lists lapsed, and the account waits until every publisher has a list in effect";
pub(super) const NO_QUORUM: &str = "no quorum of validators signed the server's validated ledger";
pub(super) const CUT: &str = "the server cut its validations short before a quorum of the account's validators; name a proof server that trusts them";
pub(super) const NO_PROOFS: &str = "the server serves no proofs; name a proof server";
pub(super) const REFUSED: &str = "the server refused to prove its ledger";
pub(super) const NOT_ANSWERED: &str = "the server did not answer";
pub(super) const OLDER: &str =
    "the server's validated ledger is older than the one the account acted on";
pub(super) const NOT_FRESH: &str = "the server's validated ledger is not fresh by the node's clock: the server is behind, or the clock is wrong";
pub(super) const NOT_PROVEN: &str = "the server's answers did not prove";
pub(super) const HISTORY: &str = "the server's history is too short to prove what the account follows; name a proof server that keeps more";

/// An anchoring round in flight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Round {
    /// A state proof at the server's validated ledger is asked for, of
    /// these keys.
    Proving(Vec<[u8; 32]>),
    /// That ledger's validations are: of the validators the server
    /// trusts, or with `untrusted`, of other keys too.
    Validating {
        proven: ProvenState,
        untrusted: bool,
    },
    /// Newer manifests of the validators that did not count are, `asked`
    /// of them still unanswered.
    Manifests {
        proven: ProvenState,
        validations: Validations,
        asked: usize,
    },
    /// The ledger is validated; a skip list that tells whether it is on the
    /// kept anchor's chain is asked for.
    Ancestry {
        anchor: Anchor,
        proven: ProvenState,
        lists: ProvenLists,
    },
}

/// Where `new` stands against the anchor kept: with none, it is taken.
pub(super) fn judge(kept: Option<&Anchor>, new: &Anchor) -> Judged {
    kept.map_or(Judged::Take, |kept| crate::anchor::judge(kept, new))
}

/// The list of recent ledgers a state proof proved: none for the first
/// ledger of a chain.
fn recent_list(proven: &ProvenState) -> Result<Option<SkipList>, Doubt> {
    proven.skip_list(&recent_key())
}

/// Why there is no UNL, for the owner.
const fn unl_reason(error: UnlError) -> &'static str {
    match error {
        UnlError::Unavailable { why, .. } => match why {
            Unavailable::NoList => NO_LIST,
            Unavailable::Expired => LIST_EXPIRED,
            Unavailable::Revoked => PUBLISHER_REVOKED,
        },
        UnlError::Blocked => UNL_BLOCKED,
        UnlError::Empty => NO_VALIDATORS,
    }
}

/// The lists ([`crate::lists`]).
impl XrplAccount {
    /// Fetches each publisher's list that is due, on a connection.
    pub(super) fn fetch_lists(&mut self) {
        if self.phase == Phase::Disconnected {
            return;
        }
        for url in self.fetches.due(&self.config.unl, self.now_ms) {
            self.effect(AssetEffect::Fetch {
                url,
                max_bytes: MAX_LIST_BYTES,
            });
        }
    }

    /// Every publisher's list is fetched at once, where none is in flight
    /// and its sites did not all fail within [`LIST_RETRY_MS`]: quorum was
    /// lost, or there is no UNL.
    pub(super) fn fetch_lists_now(&mut self) {
        self.fetches.hasten(self.now_ms);
        self.fetch_lists();
    }

    /// A fetch of `url` answered with `body`, or failed. It answers every
    /// publisher that asked that site: two publishers may share one, whose
    /// fetches the node may make as one. The list in it is a publisher's
    /// own by its key, and the others' fetch failed.
    pub(super) fn fetched(&mut self, url: &str, body: Option<&[u8]>) {
        let Some(answered) =
            (self.fetches).answered(&mut self.unl, &self.config.unl, (url, body), self.now_ms)
        else {
            return;
        };
        self.unl_changes = self.unl_changes.wrapping_add(answered.changes);
        self.fetch_lists();
        let ready = self.unl.unl(self.now_ms).is_ok();
        if ready && self.round.is_none() && self.waiting.is_some() {
            self.anchor_now();
        }
    }

    /// Fetches that took too long fail, and lists expire or take effect.
    pub(super) fn lists_on_timer(&mut self) {
        for publisher in self.fetches.late(self.now_ms) {
            if let Some(url) = self.fetches.site(&self.config.unl, publisher) {
                self.fetched(&url, None);
            }
        }
        self.tick_lists();
        self.fetch_lists();
    }

    /// Lists that expired go, and those that took effect are current.
    fn tick_lists(&mut self) {
        if crate::lists::tick(&mut self.unl, self.now_ms) {
            self.unl_changes = self.unl_changes.wrapping_add(1);
        }
    }

    /// When a fetch is due next, or one in flight is late.
    pub(super) fn lists_deadline(&self) -> Option<u64> {
        if self.phase == Phase::Disconnected {
            return None;
        }
        self.fetches.deadline()
    }
}

/// The rounds.
impl XrplAccount {
    /// Starts a round, unless one is in flight or there is no connection to
    /// the account's network yet; without a UNL, the account waits for its
    /// lists.
    pub(super) fn anchor_now(&mut self) {
        let subscribed = !matches!(
            self.phase,
            Phase::Starting {
                subscribed: false,
                ..
            }
        );
        if self.round.is_some()
            || self.walk.is_some()
            || self.phase == Phase::Disconnected
            || self.network_id.is_none()
            || !subscribed
            || self.stalled_until_ms > 0
        {
            return;
        }
        if let Err(error) = self.unl.unl(self.now_ms) {
            self.waiting = Some(unl_reason(error));
            self.tick_lists();
            self.fetch_lists_now();
            return;
        }
        self.round_wants = self.wants;
        self.round = Some(Round::Proving(self.followed_keys()));
        self.send(Request::Anchor);
    }

    /// A hint that ledger `index` closed: a round, where none is in flight
    /// and the account does not wait after a refusal, while a round is
    /// wanted, or the account is hot and its anchor is another ledger. So a
    /// hot account anchors at the ledger's own pace, and an idle one on
    /// demand. A hint that names a ledger below the anchor, as a server
    /// behind or a new chain gives, is a hint to anchor, hot or idle, and
    /// no more: only the anchor tells a reset
    /// (spec/decisions/ledger-resets.md).
    pub(super) fn anchor_on_hint(&mut self, index: u32) {
        let kept = self.anchor.map(|kept| kept.anchor.index);
        let below = kept.is_some_and(|kept| index < kept);
        let other = kept != Some(index);
        if self.wants_anchor() || below || (self.hot() && other) {
            self.anchor_now();
        }
    }

    /// Asks for a round: one starts now if it can, else on the next closed
    /// ledger, until a round that started since took an anchor.
    pub(super) fn want_anchor(&mut self) {
        self.wants = self.wants.saturating_add(1);
        self.anchor_now();
    }

    /// Asks for a round for a fresh anchor, unless one is wanted already:
    /// asked again and again, as a payment or a read waits, it costs one
    /// round.
    pub(super) fn want_fresh(&mut self) {
        if !self.wants_anchor() {
            self.want_anchor();
        }
    }

    /// Whether a round is wanted: asked for since the round that took the
    /// newest anchor started.
    pub(super) const fn wants_anchor(&self) -> bool {
        self.served < self.wants
    }

    /// Whether the account anchors on every ledger: until it is synced,
    /// while a settlement through it is expected or about to be made, and
    /// while anything waits on a proof. A transaction of ours that could be
    /// proven neither applied nor absent on this connection, which stays
    /// pending for the owner, keeps it hot no longer.
    pub(super) fn hot(&self) -> bool {
        let proving = self.pending.values().any(|(request, _)| {
            matches!(
                request,
                Request::Submit(_) | Request::TxProof(..) | Request::TxBlobs(..)
            )
        });
        self.phase != Phase::Synced
            || self.settling_until_ms > self.now_ms
            || (self.in_flight.iter()).any(|t| !self.tried.contains(&t.hash))
            || self.walk.is_some()
            || !self.branches.is_empty()
            || !self.hints.is_empty()
            || !self.lookups.is_empty()
            || proving
    }

    /// A settlement through the account is expected or about to be made
    /// until `until_ms`, at most [`MAX_SETTLING_MS`] from now.
    pub(super) fn settle_until(&mut self, until_ms: u64) {
        let until_ms = until_ms.min(self.now_ms.saturating_add(MAX_SETTLING_MS));
        self.settling_until_ms = self.settling_until_ms.max(until_ms);
    }

    /// When the heartbeat is due: [`HEARTBEAT_MS`] after the account last
    /// anchored, while it is synced, idle and wants no round already.
    pub(super) fn heartbeat_deadline(&self) -> Option<u64> {
        let idle = self.phase == Phase::Synced
            && self.round.is_none()
            && self.stalled_until_ms == 0
            && !self.wants_anchor();
        idle.then_some(self.anchored_ms.saturating_add(HEARTBEAT_MS))
    }

    /// Asserts what holds of when rounds start.
    pub(super) fn check_anchoring(&self) {
        assert!(
            self.served <= self.round_wants && self.round_wants <= self.wants,
            "a round answers what was asked before it started"
        );
        assert!(self.anchored_ms <= self.now_ms, "anchored by now");
        assert!(
            (self.taken_ms).is_none_or(|taken| taken <= self.now_ms && self.anchor.is_some()),
            "an anchor taken by now, and kept"
        );
        assert!(
            self.settling_until_ms <= self.now_ms.saturating_add(MAX_SETTLING_MS),
            "a settlement keeps the account hot for a bounded while"
        );
    }

    /// The heartbeat, if it is due: a round.
    pub(super) fn heartbeat(&mut self) {
        if self
            .heartbeat_deadline()
            .is_some_and(|due| due <= self.now_ms)
        {
            self.want_anchor();
        }
    }

    /// The round's state proof at the server's validated ledger.
    pub(super) fn proved_anchor(&mut self, outcome: Result<Value, ApiError>) {
        let Some(Round::Proving(keys)) = self.round.clone() else {
            return;
        };
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let proven = match prove_state(&answer, &keys, None) {
            Ok(proven) => proven,
            Err(error) => return self.doubted(Doubt::of(error)),
        };
        // Refused before any signature is checked.
        let new = Anchor::of(&proven.header);
        let kept = self.anchor.as_ref().map(|kept| kept.anchor);
        if judge(kept.as_ref(), &new) == Judged::Older {
            return self.wait_for_newer();
        }
        self.ask_validations(proven, self.own_publishers());
    }

    /// Whether the account's publishers are the owner's, not its network's
    /// (spec/designs/xrpl-validation.md#10-configuration-and-state): then
    /// its UNL is not the one the server follows.
    fn own_publishers(&self) -> bool {
        UnlConfig::for_network(self.network.id).as_ref() != Some(&self.config.unl)
    }

    /// Asks for the validations of the proven ledger: of the validators the
    /// server trusts, which with its network's publishers are the account's
    /// own, or with `untrusted`, of other keys too
    /// (spec/designs/xrpl-validation.md#3-the-proof-api).
    fn ask_validations(&mut self, proven: ProvenState, untrusted: bool) {
        let hash = proven.hash;
        self.round = Some(Round::Validating { proven, untrusted });
        self.send(Request::Validations { hash, untrusted });
    }

    /// The round's validations.
    pub(super) fn validated_anchor(&mut self, outcome: Result<Value, ApiError>) {
        let Some(Round::Validating { proven, untrusted }) = self.round.clone() else {
            return;
        };
        let Some(validations) = self.answer(outcome, api::validations_result) else {
            return;
        };
        self.tally(proven, validations, true, untrusted);
    }

    /// Counts the UNL's validations of the proven ledger: an anchor, or
    /// newer manifests asked for those that did not count, once a round.
    /// Short of a quorum in an answer of the server's own validators, its
    /// lists may lag the account's: the account asks for the others too,
    /// before anything else. Short of a quorum in an answer the server cut,
    /// the server chose which validations to send, and trusts others than
    /// the account's UNL: the account waits for a while, as after a
    /// refusal, and counts no lie
    /// (spec/designs/xrpl-validation.md#3-the-proof-api).
    fn tally(&mut self, proven: ProvenState, validations: Validations, ask: bool, untrusted: bool) {
        // A list that took effect since is moved in first, with the
        // manifests it carries, as rippled's `updateTrusted` moves it in
        // before a ledger's validations count.
        self.tick_lists();
        let kept = self.anchor.as_ref().map(|kept| kept.anchor);
        let anchored = self.unl.anchor(
            &proven.header,
            validations.validations.iter().map(Vec::as_slice),
            self.now_ms,
            kept.as_ref(),
            self.signatures.as_ref(),
        );
        match anchored {
            Ok(anchored) => self.anchored(&proven, anchored.anchor),
            Err(AnchorError::Older) => self.wait_for_newer(),
            Err(AnchorError::TooMany) => self.doubted(Doubt::TooMany),
            Err(AnchorError::Unl(error)) => {
                self.round = None;
                self.waiting = Some(unl_reason(error));
                self.fetch_lists_now();
            }
            // An old ledger with its old validations, as a server behind
            // gives, or a clock far off: never current, and no lie the
            // account can tell, so it waits for a newer ledger, as for one
            // older than its anchor, rather than leave the server.
            Err(AnchorError::NoQuorum(tally))
                if tally.stale > 0 && tally.counted.saturating_add(tally.stale) >= tally.quorum =>
            {
                self.count(Doubt::Older);
                self.round = None;
                self.waiting = Some(NOT_FRESH);
            }
            Err(AnchorError::NoQuorum(_)) if !untrusted => self.ask_validations(proven, true),
            Err(AnchorError::NoQuorum(tally))
                if ask && tally.unknown > 0 && !tally.missing.is_empty() =>
            {
                // Validators rotate their keys before the lists show it;
                // and a reset may come with new keys. Each round asks about
                // the next of those that did not count.
                self.fetch_lists_now();
                let count = tally.missing.len().min(MAX_MANIFEST_ASKS);
                let from = self.manifests_from % tally.missing.len();
                self.manifests_from = self.manifests_from.wrapping_add(count);
                let asked: Vec<_> = (tally.missing.iter().cycle())
                    .skip(from)
                    .take(count)
                    .copied()
                    .collect();
                self.round = Some(Round::Manifests {
                    proven,
                    validations,
                    asked: asked.len(),
                });
                for master in asked {
                    self.send(Request::Manifest(master));
                }
            }
            Err(AnchorError::NoQuorum(_)) if validations.truncated => {
                self.fetch_lists_now();
                self.stall(CUT);
            }
            Err(AnchorError::NoQuorum(_)) => {
                self.fetch_lists_now();
                self.count(Doubt::NoQuorum);
                self.stall(NO_QUORUM);
            }
        }
    }

    /// A newer manifest the server gave, or not; once the last asked
    /// answered, the validations are counted again.
    pub(super) fn manifest_read(&mut self, outcome: Result<Value, ApiError>) {
        let Some(Round::Manifests { asked, .. }) = &mut self.round else {
            return;
        };
        *asked = asked.saturating_sub(1);
        let done = *asked == 0;
        if let Some(bytes) = outcome.ok().as_ref().and_then(api::manifest_result)
            && self.unl.take_manifest(&bytes) == Ok(true)
        {
            self.unl_changes = self.unl_changes.wrapping_add(1);
        }
        if done
            && let Some(Round::Manifests {
                proven,
                validations,
                ..
            }) = self.round.take()
        {
            // Asked only of an answer with the others: one without them
            // is asked again with them first.
            self.tally(proven, validations, false, true);
        }
    }

    /// The proven ledger is validated: taken as the anchor, after its
    /// ancestry is proven if it is later than the one kept.
    fn anchored(&mut self, proven: &ProvenState, anchor: Anchor) {
        let recent = match recent_list(proven) {
            Ok(recent) => recent,
            Err(doubt) => return self.doubted(doubt),
        };
        let kept = self.anchor.as_ref().map(|kept| kept.anchor);
        match judge(kept.as_ref(), &anchor) {
            Judged::Take => self.take_anchor(anchor, proven),
            Judged::Older => self.wait_for_newer(),
            // On a network that never resets the UNL validated no older
            // chain: a ledger off the kept one is a fork, which stops the
            // account (spec/designs/xrpl-validation.md#7-resets-under-a-unl).
            Judged::Ignore if !self.network.resets() => {
                self.off_chain(anchor, proven, CHAIN_FORKED);
            }
            Judged::Ignore => {
                self.count(Doubt::OlderChain);
                self.round = None;
                self.waiting = Some(OLDER);
            }
            Judged::Reset => self.off_chain(anchor, proven, CHAIN_WENT_BACK),
            Judged::Later => {
                let mut lists = ProvenLists::new();
                lists.insert((anchor.hash, recent_key()), recent);
                self.round = Some(Round::Ancestry {
                    anchor,
                    proven: proven.clone(),
                    lists,
                });
                self.trace_ancestry();
            }
        }
    }

    /// The next step of telling a later anchor's ancestry.
    fn trace_ancestry(&mut self) {
        let (
            Some(kept),
            Some(Round::Ancestry {
                anchor,
                proven,
                lists,
            }),
        ) = (self.anchor, self.round.clone())
        else {
            return;
        };
        match ancestry(&kept, &anchor, &lists) {
            Ancestry::OnChain => self.take_anchor(anchor, &proven),
            Ancestry::OffChain => self.off_chain(anchor, &proven, CHAIN_FORKED),
            Ancestry::Lie => self.doubted(Doubt::SkipList),
            Ancestry::Prove { at, key } => self.send(Request::SkipList(at, key)),
        }
    }

    /// A skip list asked for to tell a later anchor's ancestry.
    pub(super) fn skip_list_read(
        &mut self,
        (at, key): ([u8; 32], [u8; 32]),
        outcome: Result<Value, ApiError>,
    ) {
        let Some(Round::Ancestry { lists, .. }) = &self.round else {
            return;
        };
        if lists.contains_key(&(at, key)) {
            return;
        }
        if let Err(error) = &outcome
            && error.error == "lgrNotFound"
        {
            return self.stall(HISTORY);
        }
        let Some(answer) = self.answer(outcome, api::state_proof_result) else {
            return;
        };
        let proven = prove_state(&answer, &[key], Some(&at)).map_err(Doubt::of);
        let list = match proven.and_then(|proven| proven.skip_list(&key)) {
            Ok(list) => list,
            Err(doubt) => return self.doubted(doubt),
        };
        if let Some(Round::Ancestry { lists, .. }) = &mut self.round {
            lists.insert((at, key), list);
        }
        self.trace_ancestry();
    }

    /// A validated ledger the kept anchor's chain does not hold: on a
    /// network that resets, a reset, which writes off what the books held
    /// and follows the new chain; on any other, the account stops as failed
    /// with `why`, nothing written off.
    pub(super) fn off_chain(&mut self, anchor: Anchor, proven: &ProvenState, why: &'static str) {
        self.round = None;
        // Whether a network resets is a constant of the account, never what a
        // server reports (spec/decisions/ledger-resets.md).
        if !self.network.resets() {
            // With no anchor kept, nothing can be off its chain.
            if self.anchor.is_none() {
                return self.take_anchor(anchor, proven);
            }
            self.failed = Some(why);
            self.disconnected();
            return;
        }
        let recent = recent_list(proven).ok().flatten();
        self.write_off(true);
        self.keep_anchor(anchor, recent.as_ref());
        self.validated = anchor.index;
        // Everything is read anew, on the new chain: an answer still on its
        // way may be of the chain gone.
        self.reconnect();
    }

    /// `anchor` is current: what it proves is what the account follows
    /// ([`super::history`]). State it cannot read is no anchor.
    fn take_anchor(&mut self, anchor: Anchor, proven: &ProvenState) {
        self.round = None;
        let recent = match recent_list(proven) {
            Ok(recent) => recent,
            Err(doubt) => return self.doubted(doubt),
        };
        let view = match self.read_view(anchor, proven, recent.clone()) {
            Ok(view) => view,
            Err(doubt) => return self.doubted(doubt),
        };
        self.keep_anchor(anchor, recent.as_ref());
        self.counted.anchors = self.counted.anchors.saturating_add(1);
        let absences = u64::try_from(proven.absences()).unwrap_or(u64::MAX);
        self.counted.absences = self.counted.absences.saturating_add(absences);
        self.validated = self.validated.max(anchor.index);
        self.anchored_view(view);
    }

    fn keep_anchor(&mut self, anchor: Anchor, recent: Option<&SkipList>) {
        self.anchor = Some(KeptAnchor::of(anchor, recent));
        self.waiting = None;
        self.stalled_until_ms = 0;
        self.served = self.round_wants;
        self.anchored_ms = self.now_ms;
        self.taken_ms = Some(self.now_ms);
    }

    /// The server's validated ledger is older than the anchor kept: never
    /// current. The account waits for a newer one.
    fn wait_for_newer(&mut self) {
        self.count(Doubt::Older);
        self.round = None;
        self.waiting = Some(OLDER);
    }

    /// What a proof method answered, read by `read`; `None` after a refusal,
    /// which stalls the account, or an answer that does not read, which is a
    /// lie.
    pub(super) fn answer<T>(
        &mut self,
        outcome: Result<Value, ApiError>,
        read: impl FnOnce(&Value) -> Option<T>,
    ) -> Option<T> {
        match outcome {
            Ok(result) => {
                let read = read(&result);
                if read.is_none() {
                    self.doubted(Doubt::Unreadable);
                }
                read
            }
            Err(error) => {
                self.stall(if error.error == "unknownCmd" {
                    NO_PROOFS
                } else {
                    REFUSED
                });
                None
            }
        }
    }

    /// A lie caught: counted, and the connection made anew.
    pub(super) fn doubted(&mut self, doubt: Doubt) {
        self.count(doubt);
        self.stall(NOT_PROVEN);
    }

    /// The account waits, for `why`, and connects anew after [`STALL_MS`].
    pub(super) fn stall(&mut self, why: &'static str) {
        self.round = None;
        // A walk goes with it: the next connection walks again from its
        // anchor.
        self.walk = None;
        if let Phase::CatchingUp { .. } = self.phase {
            self.phase = Phase::Starting {
                subscribed: true,
                state: true,
            };
        }
        self.waiting = Some(why);
        self.stalled_until_ms = self.now_ms.saturating_add(STALL_MS);
    }

    pub(super) fn count(&mut self, doubt: Doubt) {
        let count = self.doubts.entry(doubt).or_insert(0);
        *count = count.saturating_add(1);
    }
}
