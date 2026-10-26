//! A settlement hub inside the node simulation
//! (spec/areas/testing.md#the-simulations, spec/areas/hubs.md): n = 3f + 1
//! validators, each the node's own validator role without its IO (a
//! [`Driver`] on a database of its own, and a [`Watcher`] of the escrow on
//! its own connection to the fake XRP Ledger), whose consensus messages
//! cross the simulation's network, as the members' hub accounts reach them
//! through it. The sponsor sets the escrow up on the fake ledger before the
//! first block, from its own wallet, as the rules say.
//!
//! Faulty validators keep silent, withhold their proposals, propose what is
//! invalid or to too few to notarize it, or equivocate, as the `hub`
//! simulation's roles do; f + 1 of them may collude and fork the chain. After
//! every step of a validator its invariants hold, and no two honest
//! validators finalized different states at one height.
//!
//! At the hub's end, each validator signs a payout set alone, whenever it
//! can, and keeps its signatures as the node does; the simulation gathers
//! them over time from what the validators kept, as any member would, and
//! submits a set once a quorum signed it
//! (spec/designs/settlement-hubs.md#9-shutdown).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use dassie_hub::client::Request;
use dassie_hub::consensus::wire::{Message as Consensus, Proposal};
use dassie_hub::consensus::{Config, Evidence, Finalized, MIN_STALL_MS, PeerId, Vote, VoteKind};
use dassie_hub::escrow::{
    PayoutSet, PayoutTransaction, ValidatorKeys, escrow_master, payout_set, setup,
};
use dassie_hub::ledger::Ledger;
use dassie_hub::operation::{
    MemberOperation, Operation, SignedMemberOperation, ValidatorOperation,
};
use dassie_hub::promise::{Commitment, Fulfilled, Signed};
use dassie_hub::rules::{Fees, Limits, Rules, Terms, Timers};
use dassie_hub::service::payload::Payload;
use dassie_hub::service::wire::Message as HubMessage;
use dassie_hub::sim::Role;
use dassie_hub::validators::ValidatorIndex;
use dassie_hub::{Digest, Millis};
use dassie_identity::{NodeKey, PublicKey};
use dassie_oer::{Decode, Encode};
use dassie_xrpl::AccountId;
use dassie_xrpl::api;
use dassie_xrpl::codec::{Signer, Transact};
use dassie_xrpl::fake::FEE_BASE;

use super::{Delivery, Sim};
use crate::engine::LedgerEvent;
use crate::hub::driver::{Driver, Effects, Peer, Step};
use crate::hub::escrow::{EscrowView, Out, Watcher};
use crate::hub::transport::{MAX_EXCHANGES_PER_VALIDATOR, decode_frames};
use crate::node::Policies;
use crate::store::Store;

/// The nonce the sponsor picked for the simulated hub.
pub const NONCE: [u8; 32] = [0x4e; 32];

/// The sponsor's node key.
#[must_use]
pub fn sponsor() -> NodeKey {
    NodeKey::from_bytes(&[100; 32])
}

/// Validator `index`'s node key, the same in every run.
#[must_use]
pub fn validator_key(index: usize) -> NodeKey {
    NodeKey::from_bytes(&[u8::try_from(index + 200).unwrap(); 32])
}

/// Where validator `index` is reached.
#[must_use]
pub fn endpoint(index: usize) -> String {
    format!("v{index}.test:443")
}

/// The peer IDs of members start here, above every validator's index.
const MEMBER_PEERS: u64 = 1_000;

/// How often a validator's host looks at the escrow watcher's timers
/// (`hub/xrpl.rs`).
const WATCH_TICK_MS: u64 = 1_000;

/// The longest a validator's host waits between two ticks
/// (`hub/service.rs`).
const IDLE_TICK_MS: u64 = 1_000;

/// The least time between two fetches of a state (`hub/service.rs`).
const CATCH_UP_EVERY_MS: u64 = 1_000;

/// The most attestations waiting for room in a validator's own pool
/// (`hub/service.rs`).
const MAX_RETRIED: usize = 64;

/// How long one exchange of a member with a validator may take
/// (`engine/links.rs`).
const EXCHANGE_TIMEOUT_MS: u64 = dassie_hub::member::account::RESUBMIT_MS;

/// Where the fake ledger's deliveries to validators are kept in order:
/// above every node's index.
pub(super) const VALIDATOR_FIFO: usize = 1 << 20;

/// The hub's absence period: the design's starting value, days
/// (spec/designs/settlement-hubs.md#9-shutdown).
const ABSENCE_MS: Millis = 86_400_000;

/// The terms of the simulated hub with `validators` validators: the design's
/// starting values, with the timings derived from consensus's (Δ, the
/// precision, the margin with f faulty leaders) and from the connector's
/// message window, which the margin and the allowance for the legs between
/// members share (spec/designs/settlement-hubs.md#4-peering-over-a-hub). The
/// stall is the shortest the rules allow, twice the recovery from the
/// longest outage (`consensus/config.rs`).
#[must_use]
pub fn terms(validators: usize) -> Terms {
    let sponsor = sponsor();
    let escrow = escrow_master(&sponsor, &NONCE).unwrap().account().0;
    let faults = u64::try_from(validators - 1).unwrap() / 3;
    let config = Config::DEFAULT;
    let margin = config.margin_ms(faults);
    let window = Policies::DEFAULT.connector.message_window_ms;
    Terms {
        nonce: NONCE,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow,
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
            absence: ABSENCE_MS,
            stall: MIN_STALL_MS,
            precision: config.precision_ms,
            margin,
            allowance: window - margin,
            view_timeout: config.leader_timeout_ms,
        },
    }
}

/// The rules of the simulated hub with `validators` validators.
#[must_use]
pub fn rules(validators: usize) -> Rules {
    let terms = terms(validators);
    let acceptances = (0..validators)
        .map(|index| {
            let node = validator_key(index);
            ValidatorKeys::derive(&node, &NONCE)
                .unwrap()
                .accept(&node, &terms, endpoint(index))
                .unwrap()
        })
        .collect();
    let rules = Rules {
        terms,
        validators: acceptances,
    };
    rules.check().unwrap();
    rules
}

/// The scheme config a member gives the hub's scheme: its rules.
#[must_use]
pub fn config(validators: usize) -> String {
    let hex: String = rules(validators)
        .to_vec()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!(r#"{{"rules":"{hex}"}}"#)
}

/// One validator: its database, which outlives a crash, and while it runs
/// its driver and its watcher of the escrow.
pub struct HubValidator {
    /// Its node key.
    pub node: Arc<NodeKey>,
    /// Its database.
    pub store: Arc<Store>,
    /// Its validator, while it runs.
    pub driver: Option<Driver>,
    watcher: Option<Watcher>,
    /// What its watcher last saw of the ledger.
    view: Option<EscrowView>,
    /// Its connection to the fake ledger.
    link: Option<u64>,
    /// How far its clock is off the network's.
    pub skew_ms: i64,
    /// What it does.
    pub role: Role,
    /// Attestations its pool had no room for, to attest again.
    retried: VecDeque<ValidatorOperation>,
    /// When its watcher's timers are looked at next, on the network's
    /// clock.
    watch_at: u64,
    /// When it took its last step, on the network's clock: its host ticks
    /// it after a second without one, whatever its deadline.
    stepped_at: u64,
    /// Bumped at every start: deliveries for an earlier one are dropped.
    pub generation: u64,
    /// Whether a fetch of a state is on its way, and when the last began.
    fetching: bool,
    last_fetch: Option<u64>,
}

impl HubValidator {
    /// Whether it follows the protocol.
    #[must_use]
    pub const fn honest(&self) -> bool {
        self.role.is_honest()
    }

    /// Whether it declared the hub stalled.
    #[must_use]
    pub fn stalled(&self) -> bool {
        self.driver
            .as_ref()
            .is_some_and(|driver| driver.validator().engine().is_stalled())
    }

    /// The height of its last final block, from its database while it is
    /// down.
    #[must_use]
    pub fn height(&self, hub: &dassie_hub::HubId) -> u64 {
        match &self.driver {
            Some(driver) => driver.validator().ledger().height(),
            None => self.saved(hub).map_or(0, |ledger| ledger.height()),
        }
    }

    /// The state after its last final block, as its database holds it.
    #[must_use]
    pub fn saved(&self, hub: &dassie_hub::HubId) -> Option<Ledger> {
        let stored = self.store.hub(hub.0.as_bytes()).ok()??;
        let consensus = dassie_hub::consensus::Stored::decode_exact(&stored.consensus).ok()?;
        if stored.height == 0 {
            return None;
        }
        let root = Payload::decode_exact(&consensus.tip.block.payload)
            .ok()?
            .state;
        Ledger::restore(&stored.ledger, &root).ok()
    }
}

/// A payout set a quorum signed, and what became of it on the fake ledger.
pub struct Payout {
    /// The state after the block it pays out.
    pub ledger: Ledger,
    /// The set.
    pub set: PayoutSet,
    /// Its transactions, signed by a quorum, in sequence order.
    pub blobs: Vec<Vec<u8>>,
    /// Their hashes.
    pub hashes: Vec<[u8; 32]>,
    /// The validators whose signatures it carries.
    pub signers: Vec<usize>,
    /// The next transaction to submit.
    next: usize,
    /// Whether transactions were submitted that the ledger closing next
    /// applies.
    waiting: bool,
    /// When its first transactions were submitted, what the escrow held
    /// just before, and the ledger they went into.
    pub submitted: Option<(u64, u64, u32)>,
    /// What the escrow held once the last of them applied, and in which
    /// ledger.
    pub escrow_after: Option<(u64, u32)>,
}

/// The hub of a run.
pub struct HubNet {
    /// Its rules.
    pub rules: Rules,
    /// Its validators, by their place in the rules.
    pub validators: Vec<HubValidator>,
    /// A member's exchanges in flight with each validator.
    exchanges: BTreeMap<(usize, usize), u8>,
    /// The state root at each height, as the first honest validator to
    /// finalize it had it.
    roots: BTreeMap<u64, (usize, Digest)>,
    /// The evidence each validator reported.
    pub evidence: Vec<(usize, Evidence)>,
    /// What must never happen and did: honest validators that disagree, or
    /// evidence against an honest one.
    pub breaches: Vec<String>,
    /// Whether the colluders forked the chain already.
    pub forked: bool,
    /// What the escrow held once the sponsor had set it up, of the
    /// sponsor's own: no balance in the hub stands on it.
    pub escrow_funding: u64,
    /// Pairs of validators that cannot reach each other, the lower first.
    pub cut: BTreeSet<(usize, usize)>,
    /// The payout set submitted, once one was.
    pub payout: Option<Payout>,
    /// The connection the simulation submits a payout set on.
    submitter: u64,
    /// The dishonest member, if the run has one.
    pub liar: Option<Liar>,
}

impl HubNet {
    /// The hub's ID.
    #[must_use]
    pub fn id(&self) -> dassie_hub::HubId {
        self.rules.id()
    }

    /// f: the most faulty validators the hub tolerates.
    #[must_use]
    pub fn faults(&self) -> usize {
        (self.validators.len() - 1) / 3
    }

    /// q: the quorum.
    #[must_use]
    pub fn quorum(&self) -> usize {
        self.validators.len() - self.faults()
    }

    /// The connection the simulation submits payout sets on, and reads
    /// the fake ledger with.
    #[must_use]
    pub const fn submitter(&self) -> u64 {
        self.submitter
    }

    /// The escrow's account.
    #[must_use]
    pub fn escrow(&self) -> AccountId {
        AccountId(self.rules.terms.escrow)
    }

    /// The honest validators' indexes.
    pub fn honest(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.validators.len()).filter(|index| self.validators[*index].honest())
    }

    /// The state after the last final block of an honest validator that
    /// runs, the highest such, if any runs.
    #[must_use]
    pub fn ledger(&self) -> Option<&Ledger> {
        self.honest()
            .filter_map(|index| self.validators[index].driver.as_ref())
            .map(|driver| driver.validator().ledger())
            .max_by_key(|ledger| ledger.height())
    }

    /// The lowest height an honest validator that runs finalized.
    #[must_use]
    pub fn honest_floor(&self) -> u64 {
        self.honest()
            .filter(|index| self.validators[*index].driver.is_some())
            .map(|index| self.validators[index].height(&self.id()))
            .min()
            .unwrap_or(0)
    }
}

/// What a validator takes in a step.
// One input at a time: boxing new rules, which are rare, would buy nothing.
#[allow(clippy::large_enum_variant)]
enum Input {
    Tick,
    Message {
        from: PeerId,
        node: Option<PublicKey>,
        bytes: Vec<u8>,
    },
    Attest(ValidatorOperation),
    CatchUp(Box<Finalized>, Vec<u8>),
}

/// Where what a validator says in reply to a message goes.
#[derive(Clone, Copy)]
enum ReplyTo {
    /// Nowhere: the step took no message, or a reply.
    Nobody,
    /// Back to the validator that sent it, as messages from this one.
    Validator(usize),
    /// Back to a member's hub account.
    Member(usize),
}

/// A message from a validator to its fellows, as its role has it carried.
type Sends = Vec<(usize, Vec<u8>)>;

/// A validator's signatures of a payout set, by the escrow sequence of
/// each transaction.
type Shares = Vec<(u32, Signer)>;

impl Sim {
    /// Founds the run's hub: the sponsor funds the escrow and sets it up as
    /// the rules say, its master key disabled last; then each validator
    /// starts from genesis, with its role and a clock off by at most half
    /// the precision.
    pub(super) fn found_hub(&mut self, roles: &[Role]) {
        let count = roles.len();
        let rules = rules(count);
        let master = escrow_master(&sponsor(), &NONCE).unwrap();
        let escrow = master.account();
        self.ledger.fund(escrow);
        self.ledger.close();
        self.next_connection += 1;
        let submitter = self.next_connection;
        let sequence = account_sequence(&mut self.ledger, submitter, escrow);
        for transaction in setup(&rules, &master, sequence, FEE_BASE) {
            let result = submit(&mut self.ledger, submitter, transaction.blob());
            assert_eq!(result, "tesSUCCESS", "the sponsor's setup applies");
        }
        self.ledger.close();
        let escrow_funding = self.ledger.balance(escrow).unwrap();
        let half = i64::try_from(Config::DEFAULT.precision_ms / 2).unwrap();
        let mut validators = Vec::new();
        for (index, &role) in roles.iter().enumerate() {
            let skew_ms = i64::try_from(self.rng.next_u64() % u64::try_from(2 * half + 1).unwrap())
                .unwrap()
                - half;
            validators.push(HubValidator {
                node: Arc::new(validator_key(index)),
                store: Arc::new(Store::open_in_memory().unwrap()),
                driver: None,
                watcher: None,
                view: None,
                link: None,
                skew_ms,
                role,
                retried: VecDeque::new(),
                watch_at: 0,
                stepped_at: 0,
                generation: 0,
                fetching: false,
                last_fetch: None,
            });
        }
        self.hub = Some(Box::new(HubNet {
            rules,
            validators,
            exchanges: BTreeMap::new(),
            roots: BTreeMap::new(),
            evidence: Vec::new(),
            breaches: Vec::new(),
            forked: false,
            escrow_funding,
            cut: BTreeSet::new(),
            payout: None,
            submitter,
            liar: None,
        }));
        self.trace.push(format!(
            "{} the hub of {count} validators is founded",
            self.now
        ));
        for index in 0..count {
            self.start_validator(index);
        }
    }

    fn hub(&self) -> &HubNet {
        self.hub.as_deref().unwrap()
    }

    fn hub_mut(&mut self) -> &mut HubNet {
        self.hub.as_deref_mut().unwrap()
    }

    /// Validator `index`'s clock.
    fn validator_now(&self, index: usize) -> u64 {
        self.now
            .saturating_add_signed(self.hub().validators[index].skew_ms)
    }

    /// Starts validator `index` from its database, as its node does, and
    /// connects its watcher to the fake ledger.
    pub fn start_validator(&mut self, index: usize) {
        let now = self.validator_now(index);
        let network_now = self.now;
        let genesis = super::START;
        self.next_connection += 1;
        let connection = self.next_connection;
        let hub = self.hub_mut();
        let rules = hub.rules.clone();
        let validator = &mut hub.validators[index];
        assert!(validator.driver.is_none(), "a validator starts once");
        let driver = Driver::open(
            Arc::clone(&validator.store),
            &rules,
            genesis,
            &validator.node,
            now,
        )
        .unwrap();
        let cursor = validator.store.hub_escrow(rules.id().0.as_bytes()).unwrap();
        let watcher = Watcher::new(AccountId(rules.terms.escrow), rules.terms.network, cursor);
        validator.view = Some(driver.escrow_view());
        validator.driver = Some(driver);
        validator.watcher = Some(watcher);
        validator.link = Some(connection);
        validator.generation += 1;
        validator.retried.clear();
        validator.fetching = false;
        validator.last_fetch = None;
        let generation = validator.generation;
        validator.watch_at = network_now + WATCH_TICK_MS;
        validator.stepped_at = network_now;
        self.trace
            .push(format!("{} validator {index} starts", self.now));
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::ValidatorLedgerEvent {
                validator: index,
                generation,
                connection,
                message: None,
            },
        );
    }

    /// Crashes validator `index`: what it did not write is lost, its
    /// database stays.
    pub fn crash_validator(&mut self, index: usize) {
        let validator = &mut self.hub_mut().validators[index];
        validator.driver = None;
        validator.watcher = None;
        let link = validator.link.take();
        if let Some(connection) = link {
            self.ledger.disconnect(connection);
        }
        self.count("validator crashes");
        self.trace
            .push(format!("{} validator {index} crashes", self.now));
    }

    /// When the hub's validators are next due, on the network's clock: a
    /// validator's own deadline, at least every second, or its watcher's.
    pub(super) fn hub_deadline(&self) -> Option<u64> {
        let hub = self.hub.as_deref()?;
        hub.validators
            .iter()
            .filter(|validator| validator.driver.is_some())
            .map(|validator| {
                let driver = validator.driver.as_ref().unwrap();
                let due = driver
                    .deadline()
                    .map_or(u64::MAX, |at| at.saturating_add_signed(-validator.skew_ms));
                let idle = validator.stepped_at.saturating_add(IDLE_TICK_MS);
                due.min(idle).min(validator.watch_at).max(self.now)
            })
            .min()
    }

    /// Runs what is due of every validator.
    pub(super) fn hub_due(&mut self) {
        let Some(count) = self.hub.as_deref().map(|hub| hub.validators.len()) else {
            return;
        };
        self.liar_claims();
        for index in 0..count {
            let validator = &self.hub().validators[index];
            let Some(driver) = validator.driver.as_ref() else {
                continue;
            };
            let local = self.now.saturating_add_signed(validator.skew_ms);
            let idle = validator.stepped_at.saturating_add(IDLE_TICK_MS) <= self.now;
            if idle || driver.deadline().is_some_and(|at| at <= local) {
                self.validator_step(index, Input::Tick, ReplyTo::Nobody);
            }
            if self.hub().validators[index].watch_at <= self.now {
                self.watch_tick(index);
            }
        }
    }

    /// The watcher's timers, and an attestation that found no room, again.
    fn watch_tick(&mut self, index: usize) {
        let now = self.validator_now(index);
        let network_now = self.now;
        let validator = &mut self.hub_mut().validators[index];
        validator.watch_at = network_now + WATCH_TICK_MS;
        let (Some(watcher), Some(driver)) = (validator.watcher.as_mut(), validator.driver.as_ref())
        else {
            return;
        };
        let view = driver.escrow_view();
        watcher.tick(now, &view);
        validator.view = Some(view);
        let retried = validator.retried.pop_front();
        if let Some(operation) = retried {
            self.validator_step(index, Input::Attest(operation), ReplyTo::Nobody);
        }
        self.drain_watcher(index);
    }

    /// One step of validator `index`: written before anything of it goes
    /// out, then carried out.
    fn validator_step(&mut self, index: usize, input: Input, reply_to: ReplyTo) -> bool {
        let now = self.validator_now(index);
        let network_now = self.now;
        let validator = &mut self.hub_mut().validators[index];
        let Some(driver) = validator.driver.as_mut() else {
            return false;
        };
        validator.stepped_at = network_now;
        let (step, taken) = match input {
            Input::Tick => (driver.tick(now).unwrap(), true),
            Input::Message { from, node, bytes } => (
                driver.handle(now, from, node.as_ref(), &bytes).unwrap(),
                true,
            ),
            Input::Attest(operation) => {
                let (step, taken) = driver.attest(now, operation.clone()).unwrap();
                if !taken && validator.retried.len() < MAX_RETRIED {
                    validator.retried.push_back(operation);
                }
                (step, taken)
            }
            Input::CatchUp(finalized, state) => {
                validator.fetching = false;
                match driver.catch_up(now, *finalized, &state).unwrap() {
                    Ok(step) => (step, true),
                    Err(_) => (Step::default(), false),
                }
            }
        };
        step.write.apply(&validator.store).unwrap();
        self.carry_effects(index, step.effects, reply_to);
        self.check_validator(index);
        taken
    }

    /// Carries out what a validator's step asks, once it is written.
    fn carry_effects(&mut self, index: usize, effects: Effects, reply_to: ReplyTo) {
        let Effects {
            reply,
            sends,
            submits,
            bundles,
            behind,
            evidence,
        } = effects;
        let silent = self.hub().validators[index].role == Role::Silent;
        match reply_to {
            ReplyTo::Nobody => {}
            ReplyTo::Validator(to) if !silent => {
                for message in reply {
                    self.send_to_validator(index, to, message, true);
                }
            }
            ReplyTo::Member(member) if !silent => self.answer_member(index, member, reply),
            ReplyTo::Validator(_) | ReplyTo::Member(_) => {}
        }
        let sends = self.resolve(index, sends);
        self.route_sends(index, sends);
        for blob in submits {
            if let Some(watcher) = self.hub_mut().validators[index].watcher.as_mut() {
                watcher.submit(blob);
            }
        }
        let signed = !bundles.is_empty();
        for height in bundles {
            self.count("payout sets signed");
            self.trace.push(format!(
                "{} validator {index} signs the payout set of height {height}",
                self.now
            ));
        }
        if signed {
            self.gather_payout();
        }
        if let Some(finalized) = behind {
            self.fetch_state(index, finalized);
        }
        for found in evidence {
            self.found_evidence(index, found);
        }
        // What the ledger says of the escrow changed: the watcher hears.
        let now = self.validator_now(index);
        let validator = &mut self.hub_mut().validators[index];
        if let (Some(driver), Some(watcher)) =
            (validator.driver.as_ref(), validator.watcher.as_mut())
        {
            let view = driver.escrow_view();
            if validator.view.as_ref() != Some(&view) {
                watcher.tick(now, &view);
                validator.view = Some(view);
            }
        }
        self.drain_watcher(index);
    }

    /// Validator `index` found evidence: it must name a faulty validator
    /// only.
    fn found_evidence(&mut self, index: usize, evidence: Evidence) {
        let culprits = evidence.culprits();
        self.count("evidence found");
        self.trace.push(format!(
            "{} validator {index} has evidence against {culprits:?}",
            self.now
        ));
        let hub = self.hub_mut();
        let honest_named: Vec<usize> = culprits
            .iter()
            .map(ValidatorIndex::get)
            .filter(|named| hub.validators.get(*named).is_some_and(HubValidator::honest))
            .collect();
        if hub.validators[index].honest() && !honest_named.is_empty() {
            hub.breaches.push(format!(
                "validator {index} has evidence against honest validators {honest_named:?}"
            ));
        }
        hub.evidence.push((index, evidence));
    }

    /// The validators each send goes to, by their place in the rules.
    fn resolve(&self, index: usize, sends: Vec<(Peer, Vec<u8>)>) -> Sends {
        let hub = self.hub();
        sends
            .into_iter()
            .filter_map(|(peer, message)| {
                let to = (0..hub.validators.len())
                    .find(|other| hub.validators[*other].node.node_id() == peer.node)?;
                (to != index).then_some((to, message))
            })
            .collect()
    }

    /// Sends what validator `from` said to its fellows, as its role has it:
    /// a silent one says nothing, one that withholds keeps its proposals,
    /// an invalid one proposes what no honest validator takes, a partial one
    /// proposes to too few, an equivocating one proposes two blocks, each to
    /// half of the others, and colluders fork the chain once.
    fn route_sends(&mut self, from: usize, sends: Sends) {
        let role = self.hub().validators[from].role;
        if role == Role::Silent {
            return;
        }
        // Each message once, with everyone it goes to, in order.
        let mut grouped: Vec<(Vec<u8>, Vec<usize>)> = Vec::new();
        for (to, message) in sends {
            match grouped.iter_mut().find(|(held, _)| *held == message) {
                Some((_, recipients)) => recipients.push(to),
                None => grouped.push((message, vec![to])),
            }
        }
        for (message, recipients) in grouped {
            let proposal = match HubMessage::decode_exact(&message) {
                Ok(HubMessage::Consensus(inner)) => match Consensus::decode_message(&inner) {
                    Ok(Consensus::Proposal(proposal)) => Some(proposal),
                    _ => None,
                },
                _ => None,
            };
            match (role, proposal) {
                (Role::Withhold, Some(_)) => {}
                (Role::Partial, Some(_)) => {
                    let few = self.hub().faults();
                    for to in recipients.into_iter().take(few) {
                        self.send_to_validator(from, to, message.clone(), false);
                    }
                }
                (Role::Invalid, Some(proposal)) => {
                    let invalid = self.invalid(from, proposal);
                    for to in recipients {
                        self.send_to_validator(from, to, invalid.clone(), false);
                    }
                }
                (Role::Equivocate, Some(proposal)) => {
                    self.equivocate(from, &recipients, proposal, &[from]);
                }
                (Role::Collude, Some(proposal)) if !self.hub().forked => {
                    let colluders: Vec<usize> = (0..self.hub().validators.len())
                        .filter(|index| self.hub().validators[*index].role == Role::Collude)
                        .collect();
                    if self.equivocate(from, &recipients, proposal.clone(), &colluders) {
                        self.hub_mut().forked = true;
                    }
                }
                _ => {
                    for to in recipients {
                        self.send_to_validator(from, to, message.clone(), false);
                    }
                }
            }
        }
    }

    /// `proposal` with a payload no honest validator accepts, signed by its
    /// leader.
    fn invalid(&mut self, leader: usize, proposal: Proposal) -> Vec<u8> {
        let mut block = proposal.block;
        match block.payload.last_mut() {
            Some(byte) => *byte ^= 0x5a,
            None => block.payload.push(0x5a),
        }
        self.count("invalid proposals");
        let signature = self.sign_vote(leader, VoteKind::Notarize, &block).signature;
        hub_consensus(&Consensus::Proposal(Proposal { block, signature }))
    }

    /// Validator `index`'s vote of `kind` for `block`, signed with its
    /// consensus key.
    fn sign_vote(
        &self,
        index: usize,
        kind: VoteKind,
        block: &dassie_hub::consensus::Block,
    ) -> Vote {
        let hub = self.hub();
        let keys = ValidatorKeys::derive(&hub.validators[index].node, &NONCE).unwrap();
        let digest = block.digest(&hub.id().0);
        Vote::sign(
            &hub.id().0,
            &keys.consensus,
            ValidatorIndex::new(index).unwrap(),
            kind,
            (block.epoch, block.view),
            digest,
        )
    }

    /// Proposes `proposal` to one half of `to` and another valid block of
    /// the same height to the other half: the same parent, stamped a
    /// millisecond later, with nothing in it. Every signer in `signers` but
    /// a lone equivocator also votes to notarize and finalize each block to
    /// its half, so that with f + 1 of them each half finalizes its own.
    /// Whether it could: the leader must hold the parent's state.
    fn equivocate(
        &mut self,
        leader: usize,
        to: &[usize],
        proposal: Proposal,
        signers: &[usize],
    ) -> bool {
        let first = proposal.block.clone();
        let mut other = first.clone();
        other.timestamp_ms += 1;
        let payload = self.hub().validators[leader]
            .driver
            .as_ref()
            .and_then(|driver| {
                driver
                    .validator()
                    .empty_payload(&other.parent, other.height, other.timestamp_ms)
            });
        let Some(payload) = payload else {
            for &recipient in to {
                let bytes = hub_consensus(&Consensus::Proposal(proposal.clone()));
                self.send_to_validator(leader, recipient, bytes, false);
            }
            return false;
        };
        other.payload = payload;
        let targets: Vec<usize> = to
            .iter()
            .copied()
            .filter(|index| !signers.contains(index))
            .collect();
        let halves = [
            targets.iter().copied().step_by(2).collect::<Vec<_>>(),
            targets
                .iter()
                .copied()
                .skip(1)
                .step_by(2)
                .collect::<Vec<_>>(),
        ];
        self.count("equivocations");
        self.trace.push(format!(
            "{} validator {leader} proposes two blocks at height {} in view {}",
            self.now, first.height, first.view
        ));
        for (block, half) in [first, other].into_iter().zip(halves) {
            let signature = self.sign_vote(leader, VoteKind::Notarize, &block).signature;
            let bytes = hub_consensus(&Consensus::Proposal(Proposal {
                block: block.clone(),
                signature,
            }));
            for &recipient in &half {
                self.send_to_validator(leader, recipient, bytes.clone(), false);
            }
            if signers.len() > 1 {
                for &signer in signers {
                    for kind in [VoteKind::Notarize, VoteKind::Finalize] {
                        let vote =
                            hub_consensus(&Consensus::Vote(self.sign_vote(signer, kind, &block)));
                        for &recipient in &half {
                            self.send_to_validator(signer, recipient, vote.clone(), false);
                        }
                    }
                }
            }
        }
        true
    }

    /// Sends `message` from validator `from` to validator `to`, unless a
    /// cut or the network loses it. A reply is taken as a message from its
    /// sender, whose own reply goes nowhere, as an exchange's does.
    fn send_to_validator(&mut self, from: usize, to: usize, message: Vec<u8>, reply: bool) {
        let cut = self.hub().cut.contains(&(from.min(to), from.max(to)));
        if cut || self.rng.next_u64() % 1000 < self.loss_per_mille {
            return;
        }
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::ValidatorMessage {
                from,
                to,
                message,
                reply,
            },
        );
    }

    /// A message from validator `from` reached validator `to`.
    pub(super) fn validator_message(
        &mut self,
        from: usize,
        to: usize,
        message: Vec<u8>,
        reply: bool,
    ) {
        let node = self.hub().validators[from].node.public_key();
        let input = Input::Message {
            from: PeerId(u64::try_from(from).unwrap()),
            node: Some(node),
            bytes: message,
        };
        let reply_to = if reply {
            ReplyTo::Nobody
        } else {
            ReplyTo::Validator(from)
        };
        self.validator_step(to, input, reply_to);
    }

    /// A member's hub account sends `message` to the validator at
    /// `endpoint`, as the node's driver does: at most a few exchanges with
    /// each validator at once, each taking at most the account's interval
    /// for sending again; what does not arrive, the account asks again.
    pub(super) fn hub_send(
        &mut self,
        member: usize,
        asset: String,
        epoch: u64,
        endpoint: usize,
        message: Vec<u8>,
    ) {
        let Some(hub) = self.hub.as_deref_mut() else {
            return;
        };
        if endpoint >= hub.validators.len() {
            return;
        }
        if let Some(at) = self.late_claim(member, &message) {
            self.count("claims held back");
            self.schedule(
                at - self.now,
                Delivery::HubSubmit {
                    member,
                    endpoint,
                    message,
                },
            );
            return;
        }
        let hub = self.hub_mut();
        let slots = hub.exchanges.entry((member, endpoint)).or_default();
        if *slots >= MAX_EXCHANGES_PER_VALIDATOR {
            return;
        }
        *slots += 1;
        self.count("member exchanges");
        let generation = self.nodes[member].generation;
        if self.rng.next_u64() % 1000 < self.loss_per_mille || self.apart.contains(&member) {
            self.schedule(
                EXCHANGE_TIMEOUT_MS,
                Delivery::HubExchangeEnd { member, endpoint },
            );
            return;
        }
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::HubRequest {
                member,
                generation,
                asset,
                epoch,
                endpoint,
                message,
            },
        );
    }

    /// A member's request reached its validator, which answers it, if it
    /// runs and speaks.
    pub(super) fn hub_request(
        &mut self,
        member: usize,
        generation: u64,
        asset: String,
        epoch: u64,
        endpoint: usize,
        message: Vec<u8>,
    ) {
        let up = self.hub().validators[endpoint].driver.is_some();
        if up {
            self.answering = Some((member, generation, asset, epoch, endpoint));
            let input = Input::Message {
                from: PeerId(MEMBER_PEERS + u64::try_from(member).unwrap()),
                node: None,
                bytes: message,
            };
            self.validator_step(endpoint, input, ReplyTo::Member(member));
            self.answering = None;
        }
        let delay = if up {
            2 * self.latency_ms.1
        } else {
            EXCHANGE_TIMEOUT_MS
        };
        self.schedule(delay, Delivery::HubExchangeEnd { member, endpoint });
    }

    /// Validator `index` answers the member whose request it took.
    fn answer_member(&mut self, index: usize, member: usize, reply: Vec<Vec<u8>>) {
        let Some((asked, generation, asset, epoch, endpoint)) = self.answering.clone() else {
            return;
        };
        debug_assert!(asked == member && endpoint == index);
        for message in reply {
            if self.rng.next_u64() % 1000 < self.loss_per_mille {
                continue;
            }
            let delay = self.latency();
            self.schedule(
                delay,
                Delivery::HubAnswer {
                    member,
                    generation,
                    asset: asset.clone(),
                    epoch,
                    endpoint,
                    message,
                },
            );
        }
    }

    /// What a validator answered reaches the member's hub account.
    pub(super) fn hub_answer(
        &mut self,
        member: usize,
        generation: u64,
        asset: &str,
        epoch: u64,
        endpoint: usize,
        message: Vec<u8>,
    ) {
        if self.nodes[member].generation != generation {
            return;
        }
        let event = LedgerEvent::From { endpoint, message };
        self.step(member, |core| core.ledger_event(asset, epoch, &event));
    }

    /// A member's exchange with a validator is over: its place is free.
    pub(super) fn hub_exchange_end(&mut self, member: usize, endpoint: usize) {
        if let Some(slots) = self
            .hub
            .as_deref_mut()
            .and_then(|hub| hub.exchanges.get_mut(&(member, endpoint)))
        {
            *slots = slots.saturating_sub(1);
        }
    }

    /// Validator `index` is further behind than its peers keep blocks: it
    /// fetches the state after `finalized` from a validator that holds it,
    /// one fetch at a time and at most one a second.
    fn fetch_state(&mut self, index: usize, finalized: Box<Finalized>) {
        let now = self.now;
        let validator = &self.hub().validators[index];
        let due = validator
            .last_fetch
            .is_none_or(|at| now.saturating_sub(at) >= CATCH_UP_EVERY_MS);
        if !due || validator.fetching {
            return;
        }
        let height = finalized.block.height;
        let hub = self.hub();
        let id = hub.id();
        let state = (0..hub.validators.len())
            .filter(|other| *other != index && hub.validators[*other].driver.is_some())
            .filter(|other| {
                !hub.cut.contains(&(index.min(*other), index.max(*other)))
                    && hub.validators[*other].role != Role::Silent
            })
            .find_map(|other| {
                hub.validators[other]
                    .store
                    .hub_state(id.0.as_bytes(), height)
                    .ok()
                    .flatten()
            });
        let generation = hub.validators[index].generation;
        let validator = &mut self.hub_mut().validators[index];
        validator.last_fetch = Some(now);
        let Some(state) = state else {
            return;
        };
        validator.fetching = true;
        self.count("states fetched");
        let delay = 2 * self.latency();
        self.schedule(
            delay,
            Delivery::ValidatorState {
                validator: index,
                generation,
                finalized,
                state,
            },
        );
    }

    /// A fetched state reached validator `index`.
    pub(super) fn validator_state(
        &mut self,
        index: usize,
        generation: u64,
        finalized: Box<Finalized>,
        state: Vec<u8>,
    ) {
        if self.hub().validators[index].generation != generation {
            return;
        }
        self.validator_step(index, Input::CatchUp(finalized, state), ReplyTo::Nobody);
    }

    /// Carries out what validator `index`'s watcher decided: its requests
    /// to the fake ledger, its attestations, which the validator takes, and
    /// its cursor, which the database keeps.
    fn drain_watcher(&mut self, index: usize) {
        loop {
            let validator = &mut self.hub_mut().validators[index];
            let Some(out) = validator.watcher.as_mut().and_then(Watcher::poll) else {
                return;
            };
            match out {
                // A WebSocket loses nothing: what goes on it arrives, in
                // order, or the connection drops.
                Out::Send(message) => {
                    let Some(connection) = validator.link else {
                        continue;
                    };
                    let delay = self.latency();
                    self.schedule(
                        delay,
                        Delivery::ValidatorLedgerRequest {
                            validator: index,
                            connection,
                            message,
                        },
                    );
                }
                Out::Attest(operation, transaction) => {
                    if let Some(watcher) = validator.watcher.as_mut() {
                        watcher.taken(transaction);
                    }
                    self.trace.push(format!(
                        "{} validator {index} attests {operation:?}",
                        self.now
                    ));
                    self.validator_step(index, Input::Attest(operation), ReplyTo::Nobody);
                }
                Out::Cursor(cursor) => {
                    let hub = self.hub().id();
                    self.hub().validators[index]
                        .store
                        .set_hub_escrow(hub.0.as_bytes(), &cursor)
                        .unwrap();
                }
                Out::Problem(problem) => {
                    self.trace
                        .push(format!("{} validator {index}: {problem}", self.now));
                }
            }
        }
    }

    /// A request of validator `index`'s watcher reaches the fake ledger.
    pub(super) fn validator_ledger_request(
        &mut self,
        index: usize,
        connection: u64,
        message: &[u8],
    ) {
        let validator = &self.hub().validators[index];
        if validator.link != Some(connection) {
            return;
        }
        let generation = validator.generation;
        let reply = self.ledger.request(connection, message);
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::ValidatorLedgerEvent {
                validator: index,
                generation,
                connection,
                message: Some(reply),
            },
        );
    }

    /// What the fake ledger said reaches validator `index`'s watcher: that
    /// the connection is up, or a message.
    pub(super) fn validator_ledger_event(
        &mut self,
        index: usize,
        generation: u64,
        connection: u64,
        message: Option<Vec<u8>>,
    ) {
        let now = self.validator_now(index);
        let validator = &mut self.hub_mut().validators[index];
        if validator.generation != generation || validator.link != Some(connection) {
            return;
        }
        let (Some(watcher), Some(driver)) = (validator.watcher.as_mut(), validator.driver.as_ref())
        else {
            return;
        };
        match message {
            None => watcher.connected(),
            Some(bytes) => {
                let view = driver.escrow_view();
                watcher.message(now, &bytes, &view);
            }
        }
        self.drain_watcher(index);
    }

    /// The fake ledger's stream message on `connection`, if a validator's.
    pub(super) fn validator_stream(&mut self, connection: u64, message: Vec<u8>) -> bool {
        let Some(hub) = self.hub.as_deref() else {
            return false;
        };
        let Some(index) =
            (0..hub.validators.len()).find(|index| hub.validators[*index].link == Some(connection))
        else {
            return false;
        };
        let generation = hub.validators[index].generation;
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::ValidatorLedgerEvent {
                validator: index,
                generation,
                connection,
                message: Some(message),
            },
        );
        true
    }

    /// What holds after every step of validator `index`: its invariants,
    /// and no honest validator finalized another state at a height than the
    /// first one to reach it did.
    fn check_validator(&mut self, index: usize) {
        let hub = self.hub_mut();
        let validator = &hub.validators[index];
        let Some(driver) = validator.driver.as_ref() else {
            return;
        };
        driver.validator().check_invariants();
        if !validator.honest() {
            return;
        }
        let ledger = driver.validator().ledger();
        let height = ledger.height();
        let root = ledger.state_root();
        match hub.roots.get(&height) {
            Some((first, seen)) if *seen != root => {
                let breach = format!(
                    "honest validators {first} and {index} finalized different states at height {height}"
                );
                if !hub.breaches.contains(&breach) {
                    hub.breaches.push(breach);
                }
            }
            Some(_) => {}
            None => {
                hub.roots.insert(height, (index, root));
            }
        }
    }

    /// Gathers the payout sets the honest validators signed, as kept in
    /// their databases, over time: once a quorum signed one set, it is
    /// assembled and kept for submission. Faulty validators' signatures are
    /// not counted, nor needed.
    pub fn gather_payout(&mut self) {
        let hub = self.hub();
        if hub.payout.is_some() {
            return;
        }
        let id = hub.id();
        let quorum = hub.quorum();
        // Each honest validator's signatures, by height.
        let mut signed: BTreeMap<u64, Vec<(usize, Shares)>> = BTreeMap::new();
        for index in hub.honest() {
            for (height, bundle) in hub.validators[index]
                .store
                .hub_bundles(id.0.as_bytes())
                .unwrap()
            {
                let shares = decode_frames(&bundle)
                    .unwrap()
                    .into_iter()
                    .filter_map(|bytes| match HubMessage::decode_exact(&bytes) {
                        Ok(HubMessage::Share { sequence, signer }) => Some((sequence, signer)),
                        _ => None,
                    })
                    .collect();
                signed.entry(height).or_default().push((index, shares));
            }
        }
        // The latest block a quorum signed the set of: a member that holds
        // the proof of a later one prefers it.
        let Some((&height, signers)) = signed
            .iter()
            .rev()
            .find(|(_, signers)| signers.len() >= quorum)
        else {
            return;
        };
        // The set, from the state after its block, as any honest validator
        // that finalized it holds it.
        let ledger = hub.honest().find_map(|index| {
            let validator = &hub.validators[index];
            match &validator.driver {
                Some(driver) if driver.validator().ledger().height() == height => {
                    Some(driver.validator().ledger().clone())
                }
                _ => validator
                    .saved(&id)
                    .filter(|ledger| ledger.height() == height),
            }
        });
        let Some(ledger) = ledger else {
            return;
        };
        let set = payout_set(&ledger).unwrap();
        let signers: Vec<(usize, Shares)> = signers.iter().take(quorum).cloned().collect();
        let mut blobs = Vec::new();
        let mut hashes = Vec::new();
        for transaction in &set.transactions {
            let sequence = transaction.sequence();
            let shares: Vec<Signer> = signers
                .iter()
                .filter_map(|(_, shares)| {
                    shares
                        .iter()
                        .find(|(at, _)| *at == sequence)
                        .map(|(_, signer)| *signer)
                })
                .collect();
            assert_eq!(
                shares.len(),
                quorum,
                "every signer signed every transaction"
            );
            let (blob, hash) = match transaction {
                PayoutTransaction::Withdrawal(payment) | PayoutTransaction::Payout(payment) => {
                    let signed = payment.clone().with_signers(shares).unwrap();
                    (signed.blob, signed.hash)
                }
                PayoutTransaction::SignerList(list) | PayoutTransaction::Selector(list) => {
                    let signed = list.clone().with_signers(shares).unwrap();
                    (signed.blob, signed.hash)
                }
            };
            blobs.push(blob);
            hashes.push(hash);
        }
        let by: Vec<usize> = signers.iter().map(|(index, _)| *index).collect();
        self.trace.push(format!(
            "{} a quorum {by:?} signed the payout set of height {height}: {} transactions",
            self.now,
            set.transactions.len()
        ));
        self.count("payout sets gathered");
        self.hub_mut().payout = Some(Payout {
            ledger,
            set,
            blobs,
            hashes,
            signers: by,
            next: 0,
            waiting: false,
            submitted: None,
            escrow_after: None,
        });
    }

    /// Before the fake ledger closes: a payout set gathered goes in, in
    /// order of sequence, as any member would submit it. The XRP Ledger
    /// checks a transaction's signatures against the signer list in place
    /// when it is submitted, so the payouts, signed with the set's own keys,
    /// go in only once the selector that puts those keys on the list
    /// applied: what the escrow keys sign goes first, up to the selector,
    /// and the payouts at the next close. Each part goes once nothing else
    /// waits for the ledger.
    pub(super) fn submit_payout(&mut self) {
        let Some(hub) = self.hub.as_deref() else {
            return;
        };
        let Some(payout) = hub.payout.as_ref() else {
            return;
        };
        let count = payout.blobs.len();
        if payout.next == count || payout.waiting || self.ledger.queued() > 0 {
            return;
        }
        let (escrow, submitter) = (hub.escrow(), hub.submitter);
        let selector = payout
            .set
            .transactions
            .iter()
            .position(|transaction| matches!(transaction, PayoutTransaction::Selector(_)))
            .unwrap();
        let first = payout.next == 0;
        let until = if first { selector + 1 } else { count };
        let blobs: Vec<Vec<u8>> = payout.blobs[payout.next..until].to_vec();
        let before = self.ledger.balance(escrow).unwrap_or(0);
        for blob in &blobs {
            let result = submit(&mut self.ledger, submitter, blob);
            self.trace.push(format!(
                "{} a payout transaction is submitted: {result}",
                self.now
            ));
        }
        // What goes in now applies in the ledger after the last validated.
        let (now, ledger) = (self.now, self.ledger.ledger() + 1);
        let payout = self.hub_mut().payout.as_mut().unwrap();
        payout.next = until;
        payout.waiting = true;
        if first {
            payout.submitted = Some((now, before, ledger));
            self.count("payout sets submitted");
        }
    }

    /// Once the ledger closed: what the escrow holds after the last part of
    /// the payout set applied.
    pub(super) fn payout_closed(&mut self) {
        let Some(hub) = self.hub.as_deref() else {
            return;
        };
        let escrow = hub.escrow();
        let Some(payout) = hub.payout.as_ref() else {
            return;
        };
        if !payout.waiting {
            return;
        }
        let done = payout.next == payout.blobs.len();
        let (after, ledger) = (
            self.ledger.balance(escrow).unwrap_or(0),
            self.ledger.ledger(),
        );
        let payout = self.hub_mut().payout.as_mut().unwrap();
        payout.waiting = false;
        if done {
            payout.escrow_after = Some((after, ledger));
        }
    }
}

/// `message` as the hub's wire carries it.
fn hub_consensus(message: &Consensus) -> Vec<u8> {
    HubMessage::Consensus(message.to_vec()).to_vec()
}

/// What the fake ledger answers `request` with on `connection`.
fn request(
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    connection: u64,
    bytes: &[u8],
) -> serde_json::Value {
    match api::parse(&ledger.request(connection, bytes)) {
        api::Message::Response { outcome, .. } => outcome.unwrap(),
        other => panic!("not a response: {other:?}"),
    }
}

/// `account`'s next sequence on the fake ledger.
fn account_sequence(
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    connection: u64,
    account: AccountId,
) -> u32 {
    let result = request(ledger, connection, &api::account_info(1, account));
    api::account_info_result(&result).unwrap().sequence
}

/// Submits a signed transaction to the fake ledger: its preliminary result.
fn submit(ledger: &mut dassie_xrpl::fake::FakeLedger, connection: u64, blob: &[u8]) -> String {
    let result = request(ledger, connection, &api::submit(3, blob));
    api::submit_result(&result).unwrap()
}

/// What account `number` is paid out at the hub's end, as the design says
/// it is owed, worked out apart from the payout set: its balance and
/// reserves, and the holds it pays into, which go back to their payer.
#[must_use]
pub fn owed(ledger: &Ledger, number: dassie_hub::operation::AccountNumber) -> u64 {
    let Some(account) = ledger.account(number) else {
        return 0;
    };
    let holds: u64 = ledger
        .holds()
        .filter(|(_, hold)| hold.payer == number)
        .map(|(_, hold)| hold.amount + hold.reserve)
        .sum();
    account.balance + account.reserve + holds
}

/// A lie a member tells its peers over the hub
/// (spec/designs/settlement-hubs.md#13-how-it-fits-the-node): whatever it
/// does, it costs each peer no more than the hold between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberLie {
    /// As a payer, it sends its payees no claims, so they claim on the hub.
    NoClaims,
    /// As a payee, it keeps its Fulfills from its payers, and claims on the
    /// hub only just before the commitments expire, this long before.
    ClaimsLate(Millis),
    /// As a payee, it claims on the hub what its payers' claims cover too,
    /// and each claim twice.
    ClaimsTwice,
}

/// The dishonest member of a run, and what it saw.
pub struct Liar {
    /// Its node.
    pub node: usize,
    /// What it does.
    pub lie: MemberLie,
    /// The commitments its payers sent it, by condition.
    commitments: BTreeMap<[u8; 32], Signed<Commitment>>,
    /// The fulfillments it sent its payers, by condition, not yet claimed.
    fulfilled: BTreeMap<[u8; 32], [u8; 32]>,
    /// When it next claims of its own.
    claim_at: u64,
}

/// How often a member that claims twice claims.
const CLAIM_EVERY_MS: u64 = 2_000;

/// How long before a commitment expires a claim must reach the
/// validators to be taken in a block stamped before: a block interval and
/// three delays, as a block takes to be final (`consensus/config.rs`).
const CLAIM_LEAD_MS: u64 = Config::DEFAULT.block_interval_ms + 3 * Config::DEFAULT.max_delay_ms;

/// The most commitments and fulfillments a dishonest member keeps.
const MAX_SEEN: usize = 4_096;

impl Sim {
    /// Makes node `node` tell `lie` over the hub.
    pub fn set_liar(&mut self, node: usize, lie: MemberLie) {
        if lie == MemberLie::NoClaims {
            self.step(node, |core| {
                core.lie(crate::engine::core::Lie::NoClaims);
            });
        }
        self.hub_mut().liar = Some(Liar {
            node,
            lie,
            commitments: BTreeMap::new(),
            fulfilled: BTreeMap::new(),
            claim_at: 0,
        });
        self.trace.push(format!(
            "{} node {node} lies over the hub: {lie:?}",
            self.now
        ));
    }

    /// Looks at a peer request from `from` to `to` for the dishonest member:
    /// the commitments its payers send it and the Fulfills it sends them.
    /// Whether the request is withheld: a member that claims late keeps its
    /// Fulfills to itself.
    pub(super) fn hub_intercept(&mut self, from: usize, to: usize, body: &[u8]) -> bool {
        use dassie_peer::wire::{Content, Envelope};
        let Some(liar) = self.hub.as_deref_mut().and_then(|hub| hub.liar.as_mut()) else {
            return false;
        };
        if from != liar.node && to != liar.node {
            return false;
        }
        let Ok(envelope) = Envelope::decode(body) else {
            return false;
        };
        let Ok(content) = Content::decode_exact(envelope.content) else {
            return false;
        };
        let withheld = match content {
            Content::HubPacket { commitment, .. }
                if to == liar.node && liar.lie == MemberLie::ClaimsTwice =>
            {
                let mut reader = dassie_oer::Reader::new(commitment);
                if let Ok(signed) = Signed::<Commitment>::decode(&mut reader)
                    && liar.commitments.len() < MAX_SEEN
                {
                    liar.commitments.insert(signed.value.condition, signed);
                }
                false
            }
            Content::InterledgerPacket { packet, .. } if from == liar.node => {
                let Ok(dassie_ilp::Packet::Fulfill(fulfill)) =
                    dassie_ilp::Packet::decode_exact(packet)
                else {
                    return false;
                };
                match liar.lie {
                    MemberLie::ClaimsLate(_) => true,
                    MemberLie::ClaimsTwice => {
                        let condition = dassie_ilp::condition_for(&fulfill.fulfillment);
                        if liar.commitments.contains_key(&condition)
                            && liar.fulfilled.len() < MAX_SEEN
                        {
                            liar.fulfilled.insert(condition, fulfill.fulfillment);
                        }
                        false
                    }
                    MemberLie::NoClaims => false,
                }
            }
            _ => false,
        };
        if withheld {
            self.count("fulfills withheld");
        }
        withheld
    }

    /// When the dishonest member that claims late sends a claim of its
    /// own: just before the first of its commitments expires, if that is
    /// later than now.
    fn late_claim(&self, member: usize, message: &[u8]) -> Option<u64> {
        let liar = self.hub.as_deref()?.liar.as_ref()?;
        let MemberLie::ClaimsLate(lead) = liar.lie else {
            return None;
        };
        if liar.node != member {
            return None;
        }
        let Ok(HubMessage::Request(Request::Submit(Operation::Member(signed)))) =
            HubMessage::decode_exact(message)
        else {
            return None;
        };
        let MemberOperation::Claim { fulfilled, .. } = &signed.operation else {
            return None;
        };
        let expiry = fulfilled
            .iter()
            .map(|item| item.commitment.value.expiry)
            .min()?;
        let at = expiry.saturating_sub(lead);
        (at > self.now).then_some(at)
    }

    /// The member that claims twice claims on the hub what its payers'
    /// claims may cover too, every few seconds: the commitments it took
    /// and fulfilled that have not expired, and then the same claim again.
    pub(super) fn liar_claims(&mut self) {
        let Some(hub) = self.hub.as_deref() else {
            return;
        };
        let Some(liar) = hub.liar.as_ref() else {
            return;
        };
        if liar.lie != MemberLie::ClaimsTwice || liar.claim_at > self.now {
            return;
        }
        let member = liar.node;
        let Some(ledger) = hub.ledger() else {
            return;
        };
        let id = hub.id();
        let key = dassie_hub::escrow::member_key(&super::key(member), &id).unwrap();
        let Some(number) = ledger.account_of(&key.public_key()) else {
            return;
        };
        let sequence = ledger.account(number).unwrap().sequence;
        let limit = usize::from(ledger.terms().limits.commitments_per_claim);
        let fee = ledger.terms().fees.operation;
        let soon = self.now + CLAIM_LEAD_MS;
        let pairs: Vec<Fulfilled> = liar
            .fulfilled
            .iter()
            .filter_map(|(condition, fulfillment)| {
                let commitment = liar.commitments.get(condition)?;
                (commitment.value.expiry > soon).then_some(Fulfilled {
                    commitment: *commitment,
                    fulfillment: *fulfillment,
                })
            })
            .collect();
        let validators = hub.validators.len();
        let now = self.now;
        let liar = self.hub_mut().liar.as_mut().unwrap();
        liar.claim_at = now + CLAIM_EVERY_MS;
        liar.fulfilled.clear();
        let Some(hold) = pairs.first().map(|pair| pair.commitment.value.hold) else {
            return;
        };
        let fulfilled: Vec<Fulfilled> = pairs
            .into_iter()
            .filter(|pair| pair.commitment.value.hold == hold)
            .take(limit)
            .collect();
        self.count("claims made twice");
        self.trace.push(format!(
            "{} node {member} claims {} packets on hold {} twice",
            self.now,
            fulfilled.len(),
            hold.0
        ));
        for (offset, delay) in [(0, 0), (1, CLAIM_EVERY_MS / 2)] {
            let operation = Operation::Member(SignedMemberOperation::sign(
                &id,
                number,
                sequence + offset,
                fee,
                MemberOperation::Claim {
                    hold,
                    fulfilled: fulfilled.clone(),
                },
                &key,
            ));
            let message = HubMessage::Request(Request::Submit(operation)).to_vec();
            for endpoint in 0..validators {
                let latency = self.latency();
                self.schedule(
                    delay + latency,
                    Delivery::HubSubmit {
                        member,
                        endpoint,
                        message: message.clone(),
                    },
                );
            }
        }
    }

    /// A member's submission reaches a validator outside its account's
    /// exchanges: it is answered by no one.
    pub(super) fn hub_submit(&mut self, member: usize, endpoint: usize, message: Vec<u8>) {
        if self.hub().validators[endpoint].driver.is_none() {
            return;
        }
        let input = Input::Message {
            from: PeerId(MEMBER_PEERS + u64::try_from(member).unwrap()),
            node: None,
            bytes: message,
        };
        self.validator_step(endpoint, input, ReplyTo::Nobody);
    }
}
