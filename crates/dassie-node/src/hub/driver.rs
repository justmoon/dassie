//! One hub's validator in the node, without its IO: a
//! `dassie_hub::service::Validator`, what it persisted, and what each step
//! asks the node to do.
//!
//! Every input is a step: a message, the time, or something seen on the
//! escrow. A step folds its consensus batches into the stored state, and
//! returns what to write, with the ledger after a block that became final,
//! in one transaction ([`Write`]), and its actions ([`Effects`]). Its host
//! writes first, off the async runtime, and only then carries the actions
//! out: a validator never sends a vote it could forget in a crash, and
//! comes back from the state after its last final block
//! (spec/areas/hubs.md#consensus). A write that fails ends the step with
//! nothing carried out.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use dassie_hub::consensus::{Evidence, Finalized, PeerId, Stored};
use dassie_hub::escrow::{PayoutSet, ValidatorKeys};
use dassie_hub::ledger::Ledger;
use dassie_hub::operation::ValidatorOperation;
use dassie_hub::rules::Rules;
use dassie_hub::service::payload::Payload;
use dassie_hub::service::wire::Message;
use dassie_hub::service::{Action, CatchUpError, MAX_WAITING_DEPOSITS, Validator, genesis_block};
use dassie_hub::{HubId, Millis};
use dassie_identity::{NodeId, NodeKey, PublicKey};
use dassie_oer::{Decode, Encode};
use dassie_xrpl::codec::Signer;

use super::escrow::EscrowView;
use super::transport::{HubStatus, encode_frames};
use crate::store::{HubStep, Store, StoreError};

/// A validator to send to: its node, and where it is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Its node key, which the connection is pinned to.
    pub key: PublicKey,
    /// Its node.
    pub node: NodeId,
    /// Its endpoint, `host:port`.
    pub endpoint: String,
}

/// What a step asks the node to do, once it is on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// What goes back to whoever sent the message the step took, in order.
    pub reply: Vec<Vec<u8>>,
    /// Messages to other validators, each an exchange of its own.
    pub sends: Vec<(Peer, Vec<u8>)>,
    /// Escrow transactions a quorum signed, for the XRP Ledger.
    pub submits: Vec<Vec<u8>>,
    /// Payout bundles signed: the height each pays out from.
    pub bundles: Vec<u64>,
    /// We are further behind than peers keep blocks: the final block whose
    /// state to fetch from a validator, for [`Driver::catch_up`].
    pub behind: Option<Box<Finalized>>,
    /// Evidence that validators broke the rules, which the step logged.
    pub evidence: Vec<Evidence>,
}

/// What a step writes before any of its actions go out: one transaction
/// of its consensus state, the state after a block that became final and
/// the blocks that ended epochs, then the payout bundles it signed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Write {
    hub: [u8; 32],
    now: Millis,
    consensus: Option<Vec<u8>>,
    ledger: Option<(u64, Vec<u8>)>,
    boundaries: Vec<u64>,
    bundles: Vec<(u64, Vec<u8>)>,
}

impl Write {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.consensus.is_none()
            && self.ledger.is_none()
            && self.boundaries.is_empty()
            && self.bundles.is_empty()
    }

    /// Writes it: a blocking call, for a task that may block.
    ///
    /// # Errors
    ///
    /// The store failed.
    pub fn apply(&self, store: &Store) -> Result<(), StoreError> {
        let step = HubStep {
            consensus: self.consensus.as_deref(),
            ledger: self
                .ledger
                .as_ref()
                .map(|(height, state)| (*height, state.as_slice())),
            boundaries: &self.boundaries,
        };
        store.save_hub_step(&self.hub, self.now, step)?;
        for (height, bundle) in &self.bundles {
            store.add_hub_bundle(&self.hub, *height, self.now, bundle)?;
        }
        Ok(())
    }
}

/// A step: what to write, then what to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// What to write first.
    pub write: Write,
    /// What to do once it is written.
    pub effects: Effects,
}

/// Why a hub's validator cannot run.
#[derive(Debug)]
pub enum DriverError {
    /// The node is not one of the hub's validators.
    NotAValidator,
    /// What the node stored for the hub does not read, or does not fit
    /// together.
    Stored(&'static str),
    /// The database failed.
    Store(StoreError),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAValidator => f.write_str("the node is not one of the hub's validators"),
            Self::Stored(what) => write!(f, "what the node stored for the hub is broken: {what}"),
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl From<StoreError> for DriverError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// The most deposits recorded that the escrow's view names: one that
/// leaves it before the watcher saw it is attested again, and named again.
pub const MAX_RECORDED_SHOWN: usize = 256;

/// One hub's validator and what it persisted.
pub struct Driver {
    hub: HubId,
    own: NodeId,
    validator: Validator,
    stored: Stored,
    /// The height of the ledger last written.
    written_height: u64,
    /// How often it took a peer's state.
    caught_up: u64,
    store: Arc<Store>,
    /// Deposits attested, by transaction, with their validated ledger, until
    /// the ledger recorded the attestation: at most
    /// [`MAX_WAITING_DEPOSITS`].
    awaited: BTreeMap<[u8; 32], u32>,
    /// The newest deposits whose attestation the ledger recorded, for the
    /// escrow's watcher: at most [`MAX_RECORDED_SHOWN`].
    recorded: VecDeque<[u8; 32]>,
    /// The ledger's height when `awaited` was last looked at, unless it
    /// changed since.
    awaited_at: Option<u64>,
    /// When the ledger last wrote off a reset test network's escrow, as of
    /// `awaited` and `recorded`: once it writes off again, the deposits
    /// they hold are the old chain's, which no ledger records any more.
    chain: Option<Millis>,
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver")
            .field("hub", &self.hub)
            .field("height", &self.validator.ledger().height())
            .finish_non_exhaustive()
    }
}

impl Driver {
    /// The validator of the hub of `rules` for the node of `node_key`: as
    /// it was stored, or from genesis at `genesis_ms` if it never ran, and
    /// then stored before anything else happens.
    ///
    /// # Errors
    ///
    /// See [`DriverError`].
    pub fn open(
        store: Arc<Store>,
        rules: &Rules,
        genesis_ms: Millis,
        node_key: &NodeKey,
        now: Millis,
    ) -> Result<Self, DriverError> {
        let keys = ValidatorKeys::derive(node_key, rules.terms.network, &rules.terms.nonce)
            .ok_or(DriverError::NotAValidator)?;
        let hub = rules.id();
        let own = node_key.node_id();
        if let Some(saved) = store.hub(hub.0.as_bytes())? {
            let stored = Stored::decode_exact(&saved.consensus)
                .map_err(|_| DriverError::Stored("its consensus state"))?;
            if saved.height != stored.tip.block.height {
                return Err(DriverError::Stored("its ledger is not its tip's"));
            }
            let ledger = stored_ledger(&stored, &saved.ledger, rules, genesis_ms)?;
            if *ledger.hub() != hub {
                return Err(DriverError::Stored("its ledger is another hub's"));
            }
            let validator = Validator::resume(keys, stored.clone(), ledger, now)
                .map_err(|_| DriverError::Stored("its ledger is not its tip's"))?;
            let chain = validator.ledger().written_off_at();
            return Ok(Self {
                hub,
                own,
                validator,
                stored,
                written_height: saved.height,
                caught_up: 0,
                store,
                awaited: BTreeMap::new(),
                recorded: VecDeque::new(),
                awaited_at: None,
                chain,
            });
        }
        let validator = Validator::genesis(rules.clone(), keys, genesis_ms, now)
            .map_err(|_| DriverError::NotAValidator)?;
        let set = validator.ledger().validators().clone();
        let stored = Stored::genesis(genesis_block(rules, genesis_ms), set);
        let step = HubStep {
            consensus: Some(&stored.to_vec()),
            ledger: Some((0, &validator.ledger().snapshot())),
            boundaries: &[],
        };
        store.save_hub_step(hub.0.as_bytes(), now, step)?;
        Ok(Self {
            hub,
            own,
            validator,
            stored,
            written_height: 0,
            caught_up: 0,
            store,
            awaited: BTreeMap::new(),
            recorded: VecDeque::new(),
            awaited_at: None,
            chain: None,
        })
    }

    /// The hub.
    #[must_use]
    pub const fn hub(&self) -> &HubId {
        &self.hub
    }

    /// The store its steps are written to.
    #[must_use]
    pub const fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// The validator.
    #[must_use]
    pub const fn validator(&self) -> &Validator {
        &self.validator
    }

    /// When [`tick`](Self::tick) is due next.
    #[must_use]
    pub fn deadline(&self) -> Option<Millis> {
        self.validator.poll_timeout()
    }

    /// Takes a message from `from`, whose node key the transport proved,
    /// if it did.
    ///
    /// # Errors
    ///
    /// The step's state is over its bounds: nothing of it goes out.
    pub fn handle(
        &mut self,
        now: Millis,
        from: PeerId,
        node: Option<&PublicKey>,
        message: &[u8],
    ) -> Result<Step, StoreError> {
        self.validator.handle(now, from, node, message);
        self.finish(now, Some(from))
    }

    /// Takes the state after `finalized`, fetched from a validator: only
    /// if it is the state the block names. The new state is written as any
    /// step's.
    ///
    /// # Errors
    ///
    /// The state was not taken, and nothing changed; or the step's state
    /// is over its bounds, and nothing of it goes out.
    pub fn catch_up(
        &mut self,
        now: Millis,
        finalized: Finalized,
        snapshot: &[u8],
    ) -> Result<Result<Step, CatchUpError>, StoreError> {
        match self.validator.catch_up(now, finalized, snapshot) {
            Ok(()) => {
                self.caught_up = self.caught_up.saturating_add(1);
                self.finish(now, None).map(Ok)
            }
            Err(error) => Ok(Err(error)),
        }
    }

    /// Does what is due.
    ///
    /// # Errors
    ///
    /// The step's state is over its bounds: nothing of it goes out.
    pub fn tick(&mut self, now: Millis) -> Result<Step, StoreError> {
        self.validator.tick(now);
        self.finish(now, None)
    }

    /// Attests what the node saw of the escrow, and whether the validator
    /// took it: one its pool had no room for is attested again later. A
    /// deposit is followed until the ledger recorded its attestation
    /// ([`EscrowView::deposits_recorded`]); the issuer's hold, proven by the
    /// node's own watcher, also counts for signing the payout set of a hub
    /// that stopped while the issuer held it, which makes no block that
    /// could show the thaw (spec/areas/hubs.md#payout-bundles).
    ///
    /// # Errors
    ///
    /// The step's state is over its bounds: nothing of it goes out.
    pub fn attest(
        &mut self,
        now: Millis,
        operation: ValidatorOperation,
    ) -> Result<(Step, bool), StoreError> {
        match &operation {
            ValidatorOperation::AttestDeposit(deposit) => {
                if self.awaited.len() < MAX_WAITING_DEPOSITS {
                    self.awaited.insert(deposit.transaction.0, deposit.ledger);
                    self.awaited_at = None;
                }
            }
            ValidatorOperation::AttestIssuer(issuer) => self.validator.prove_issuer(now, *issuer),
            _ => {}
        }
        let taken = self.validator.attest(now, operation);
        Ok((self.finish(now, None)?, taken))
    }

    /// Moves the deposits whose attestation the ledger recorded, or takes
    /// no more, to those the escrow's view names: each once the ledger's
    /// height changed, or one was attested, since they were last looked
    /// at. Once the hub ended, or while the node validates in no set in
    /// force, none can be recorded, and none is waited for. Once the ledger
    /// wrote off a reset test network's escrow, the old chain's deposits
    /// are waited for and named no more: the watcher attests again what it
    /// reads on the new chain.
    fn settle_deposits(&mut self) {
        let ledger = self.validator.ledger();
        let height = ledger.height();
        if ledger.written_off_at() != self.chain {
            self.chain = ledger.written_off_at();
            self.awaited.clear();
            self.recorded.clear();
        }
        if self.awaited.is_empty() || self.awaited_at == Some(height) {
            return;
        }
        self.awaited_at = Some(height);
        let index = self.validator.index_in_force();
        let ended = ledger.phase() == dassie_hub::ledger::Phase::Ended;
        let done: Vec<[u8; 32]> = (self.awaited.iter())
            .filter(|(hash, at)| {
                ended
                    || index.is_none_or(|index| {
                        ledger.deposit_recorded(index, &dassie_hub::Digest(**hash), **at)
                    })
            })
            .map(|(hash, _)| *hash)
            .collect();
        for hash in done {
            self.awaited.remove(&hash);
            if self.recorded.len() >= MAX_RECORDED_SHOWN {
                self.recorded.pop_front();
            }
            self.recorded.push_back(hash);
        }
    }

    /// What the step writes, and its actions.
    fn finish(&mut self, now: Millis, from: Option<PeerId>) -> Result<Step, StoreError> {
        let mut write = self.persist(now)?;
        self.settle_deposits();
        let mut effects = Effects::default();
        while let Some(action) = self.validator.poll() {
            match action {
                Action::Peer(peer, message) if Some(peer) == from => effects.reply.push(message),
                // Nothing is pushed: a member hears only in replies.
                Action::Peer(..) => {}
                Action::Validators(message) => {
                    for peer in self.peers() {
                        effects.sends.push((peer, message.clone()));
                    }
                }
                Action::Validator(key, message) => {
                    if let Some(peer) = self.peer_of(&key) {
                        effects.sends.push((peer, message));
                    }
                }
                Action::Submit(blob) => effects.submits.push(blob),
                Action::Payout(set, signatures) => {
                    write
                        .bundles
                        .push((set.height, self.bundle(&set, &signatures)));
                    effects.bundles.push(set.height);
                }
                Action::Behind(finalized) => effects.behind = Some(finalized),
                Action::Evidence(evidence) => {
                    tracing::warn!(
                        target: crate::log::target::HUB,
                        hub = %hex(self.hub.0.as_bytes()),
                        culprits = ?evidence.culprits(),
                        "validators of the hub broke the rules: evidence names them"
                    );
                    effects.evidence.push(*evidence);
                }
            }
        }
        Ok(Step { write, effects })
    }

    /// Folds the step's batches into the stored state, and what to write
    /// of it, with the ledger if a block became final.
    fn persist(&mut self, now: Millis) -> Result<Write, StoreError> {
        let mut write = Write {
            hub: *self.hub.0.as_bytes(),
            now,
            ..Write::default()
        };
        let mut changed = false;
        let mut boundaries = Vec::new();
        for batch in self.validator.take_batches() {
            changed |= !batch.is_empty();
            boundaries.extend(batch.boundaries.iter().map(|b| b.block.height));
            self.stored.apply(batch);
        }
        let ledger = self.validator.ledger();
        let height = ledger.height();
        assert_eq!(
            height, self.stored.tip.block.height,
            "the ledger and the stored tip are at one height after a step"
        );
        if !changed && height == self.written_height {
            return Ok(write);
        }
        if !self.stored.fits() {
            return Err(StoreError::Corrupt("hub consensus state over its bounds"));
        }
        write.consensus = changed.then(|| self.stored.to_vec());
        write.ledger = (height != self.written_height).then(|| (height, ledger.snapshot()));
        write.boundaries = boundaries;
        self.written_height = height;
        Ok(write)
    }

    /// Every other validator: of the set in force, and of new rules
    /// waiting to take over, each once.
    #[must_use]
    pub fn peers(&self) -> Vec<Peer> {
        let ledger = self.validator.ledger();
        let mut peers: Vec<Peer> = Vec::new();
        let current = ledger.rules().validators.iter().enumerate().map(|(i, a)| {
            let endpoint = dassie_hub::ValidatorIndex::new(i)
                .and_then(|index| ledger.validator(index))
                .map_or_else(|| a.endpoint.clone(), |state| state.endpoint.clone());
            (a.node, endpoint)
        });
        let pending = ledger
            .pending_rules()
            .into_iter()
            .flat_map(|pending| pending.rules.validators.iter())
            .map(|a| (a.node, a.endpoint.clone()));
        for (key, endpoint) in current.chain(pending) {
            let node = key.node_id();
            if node != self.own && !peers.iter().any(|peer| peer.node == node) {
                peers.push(Peer {
                    key,
                    node,
                    endpoint,
                });
            }
        }
        peers
    }

    /// The other validators' nodes: what they send has a queue of its own.
    #[must_use]
    pub fn peer_nodes(&self) -> std::collections::BTreeSet<NodeId> {
        self.peers().into_iter().map(|peer| peer.node).collect()
    }

    /// The validator whose consensus key is `key`.
    fn peer_of(&self, key: &dassie_identity::PublicKey) -> Option<Peer> {
        let ledger = self.validator.ledger();
        let node = ledger
            .rules()
            .validators
            .iter()
            .chain(
                ledger
                    .pending_rules()
                    .into_iter()
                    .flat_map(|p| p.rules.validators.iter()),
            )
            .find(|a| a.consensus == *key)?
            .node
            .node_id();
        self.peers().into_iter().find(|peer| peer.node == node)
    }

    /// Our signatures of a payout set, to keep retrievable as the hub's
    /// `Share` messages in sequence order (spec/designs/settlement-hubs.md#9-shutdown).
    fn bundle(&self, set: &PayoutSet, signatures: &[Signer]) -> Vec<u8> {
        let shares: Vec<Vec<u8>> = set
            .transactions
            .iter()
            .zip(signatures)
            .map(|(transaction, signer)| {
                Message::Share {
                    sequence: transaction.sequence(),
                    signer: *signer,
                }
                .to_vec()
            })
            .collect();
        tracing::info!(
            target: crate::log::target::HUB,
            hub = %hex(self.hub.0.as_bytes()),
            height = set.height,
            transactions = set.transactions.len(),
            "signed the hub's payout set"
        );
        encode_frames(&shares)
    }

    /// What the escrow's watcher needs of the ledger.
    #[must_use]
    pub fn escrow_view(&self) -> EscrowView {
        let ledger = self.validator.ledger();
        let escrow = ledger.escrow();
        EscrowView {
            rules: ledger.rules().clone(),
            checked: escrow.checked,
            next_sequence: escrow.next_sequence,
            withdrawals: ledger
                .withdrawals()
                .map(|(sequence, withdrawal)| (sequence, withdrawal.clone()))
                .collect(),
            signer_list: escrow.signer_list,
            written_off_at: ledger.written_off_at(),
            issuer: *ledger.issuer(),
            deposit_room: match self.validator.index_in_force() {
                Some(index) if ledger.phase() != dassie_hub::ledger::Phase::Ended => {
                    ledger.deposit_room(index)
                }
                _ => 0,
            },
            deposits_recorded: self.recorded.iter().copied().collect(),
        }
    }

    /// How the validator stands, for its owner.
    #[must_use]
    pub fn status(&self, xrpl_connected: bool) -> HubStatus {
        let ledger = self.validator.ledger();
        HubStatus {
            hub: self.hub,
            index: self.validator.index_in_force().map(|index| index.get()),
            validators: ledger.validators().len(),
            height: ledger.height(),
            timestamp_ms: ledger.timestamp(),
            view: self.validator.engine().view(),
            phase: ledger.phase(),
            stalled: self.validator.engine().is_stalled(),
            escrow_checked: ledger.escrow().checked,
            xrpl_connected,
            caught_up: self.caught_up,
        }
    }
}

/// The ledger a validator stored after its tip: the genesis state at
/// height 0, otherwise its snapshot, restored against the root the tip
/// names.
fn stored_ledger(
    stored: &Stored,
    snapshot: &[u8],
    rules: &Rules,
    genesis_ms: Millis,
) -> Result<Ledger, DriverError> {
    if stored.tip.block.height == 0 {
        return Ledger::genesis(rules.clone(), genesis_ms)
            .map_err(|_| DriverError::Stored("its genesis"));
    }
    let root = Payload::decode_exact(&stored.tip.block.payload)
        .map_err(|_| DriverError::Stored("its tip"))?
        .state;
    Ledger::restore(snapshot, &root).map_err(|_| DriverError::Stored("its ledger"))
}

/// Bytes as lowercase hex, for logs.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    //! Four validators' drivers, each on a database of its own on disk,
    //! with their messages delivered at once: they finalize blocks, every
    //! one restarts from what it wrote, and the hub goes on
    //! (spec/areas/hubs.md#consensus).

    use std::collections::VecDeque;

    use super::*;
    use crate::hub::config::tests::rules;

    const START: Millis = 1_800_000_000_000;

    struct Net {
        dirs: Vec<tempfile::TempDir>,
        nodes: Vec<NodeKey>,
        rules: Rules,
        drivers: Vec<Driver>,
        queue: VecDeque<(usize, usize, Vec<u8>)>,
        now: Millis,
        /// Escrow transactions each step handed the XRPL connection, by
        /// validator.
        submits: Vec<(usize, Vec<u8>)>,
        /// Whether escrow signatures between validators wait in `held`.
        hold_shares: bool,
        held: Vec<(usize, usize, Vec<u8>)>,
        /// Payout sets each step signed: the validator, and the height.
        bundles: Vec<(usize, u64)>,
        /// Whether the validators are cut off from each other.
        cut: bool,
    }

    impl Net {
        fn new() -> Self {
            Self::with_terms(|_| {})
        }

        /// A network of a hub whose terms `adjust` changes, each validator
        /// accepting them.
        fn with_terms(adjust: impl FnOnce(&mut dassie_hub::rules::Terms)) -> Self {
            let nodes: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
            let endpoints: Vec<(NodeKey, String)> = nodes
                .iter()
                .enumerate()
                .map(|(i, key)| {
                    (
                        NodeKey::from_bytes(key.as_bytes()),
                        format!("v{i}.example:443"),
                    )
                })
                .collect();
            let mut rules = rules(&NodeKey::from_bytes(&[100; 32]), &endpoints);
            adjust(&mut rules.terms);
            let terms = rules.terms.clone();
            for ((node, endpoint), acceptance) in endpoints.iter().zip(&mut rules.validators) {
                *acceptance = ValidatorKeys::derive(node, terms.network, &terms.nonce)
                    .unwrap()
                    .accept(node, &terms, endpoint.clone())
                    .unwrap();
            }
            let dirs: Vec<tempfile::TempDir> =
                (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
            let mut net = Self {
                dirs,
                nodes,
                rules,
                drivers: Vec::new(),
                queue: VecDeque::new(),
                now: START,
                submits: Vec::new(),
                hold_shares: false,
                held: Vec::new(),
                bundles: Vec::new(),
                cut: false,
            };
            net.open();
            net
        }

        /// Opens every validator from its database, as a node does at
        /// start.
        fn open(&mut self) {
            self.drivers.clear();
            for (dir, key) in self.dirs.iter().zip(&self.nodes) {
                let store = Arc::new(Store::open(dir.path()).unwrap());
                let driver = Driver::open(store, &self.rules, START, key, self.now).unwrap();
                self.drivers.push(driver);
            }
        }

        fn index_of(&self, node: &NodeId) -> usize {
            self.nodes
                .iter()
                .position(|key| key.node_id() == *node)
                .unwrap()
        }

        fn deliver(&mut self, from: usize, step: Step) {
            step.write.apply(self.drivers[from].store()).unwrap();
            for blob in step.effects.submits {
                self.submits.push((from, blob));
            }
            for height in step.effects.bundles {
                self.bundles.push((from, height));
            }
            if self.cut {
                return;
            }
            for (peer, message) in step.effects.sends {
                let to = self.index_of(&peer.node);
                let share = matches!(Message::decode_exact(&message), Ok(Message::Share { .. }));
                if self.hold_shares && share {
                    self.held.push((to, from, message));
                } else {
                    self.queue.push_back((to, from, message));
                }
            }
        }

        fn run(&mut self, ms: u64) {
            for _ in 0..ms {
                self.now += 1;
                for i in 0..self.drivers.len() {
                    if self.drivers[i].deadline().is_some_and(|at| at <= self.now) {
                        let step = self.drivers[i].tick(self.now).unwrap();
                        self.deliver(i, step);
                    }
                }
                while let Some((to, from, message)) = self.queue.pop_front() {
                    let peer = PeerId(u64::try_from(from).unwrap());
                    let node = self.nodes[from].public_key();
                    let step = self.drivers[to]
                        .handle(self.now, peer, Some(&node), &message)
                        .unwrap();
                    for reply in &step.effects.reply {
                        self.queue.push_back((from, to, reply.clone()));
                    }
                    self.deliver(to, step);
                }
            }
        }

        fn heights(&self) -> Vec<u64> {
            self.drivers
                .iter()
                .map(|d| d.validator().ledger().height())
                .collect()
        }
    }

    #[test]
    fn validators_restart_from_what_they_wrote() {
        let mut net = Net::new();
        for i in 0..net.drivers.len() {
            let attest = ValidatorOperation::AttestEscrow { sequence: 5 };
            let (step, taken) = net.drivers[i].attest(net.now, attest).unwrap();
            assert!(taken);
            net.deliver(i, step);
        }
        net.run(2_000);
        let before = net.heights();
        assert!(before.iter().all(|height| *height > 5), "{before:?}");
        assert!(net.drivers.iter().all(|d| d.escrow_view().checked));

        // Every validator stops at once, its database closed, and starts
        // again from it.
        net.open();
        assert_eq!(
            net.heights(),
            before,
            "each resumes at its last final block"
        );
        assert!(net.drivers.iter().all(|d| d.escrow_view().checked));
        net.run(2_000);
        let after = net.heights();
        assert!(
            after.iter().zip(&before).all(|(a, b)| *a > b + 5),
            "{before:?} → {after:?}"
        );
        let low = *after.iter().min().unwrap();
        let roots: Vec<_> = net
            .drivers
            .iter()
            .filter(|d| d.validator().ledger().height() == low)
            .map(|d| d.validator().ledger().state_root())
            .collect();
        assert!(roots.windows(2).all(|w| w[0] == w[1]));
    }

    /// Every validator attests `operation`.
    fn attest_all(net: &mut Net, operation: &ValidatorOperation) {
        for i in 0..net.drivers.len() {
            let (step, taken) = net.drivers[i].attest(net.now, operation.clone()).unwrap();
            assert!(taken);
            net.deliver(i, step);
        }
    }

    /// A member's operation, sent to every validator.
    fn submit(net: &mut Net, operation: &dassie_hub::operation::Operation) {
        let bytes = Message::Operation(operation.clone()).to_vec();
        for i in 0..net.drivers.len() {
            let step = net.drivers[i]
                .handle(net.now, PeerId(100), None, &bytes)
                .unwrap();
            net.deliver(i, step);
        }
    }

    /// A DSUSD hub with a member's withdrawal sequenced, whose validators'
    /// signatures of it wait in `held`.
    fn token_hub_withdrawing() -> Net {
        use dassie_hub::digest::Digest;
        use dassie_hub::operation::{
            Deposit, Destination, MemberOperation, Operation, SignedMemberOperation,
        };

        let mut net = Net::with_terms(|terms| {
            terms.asset = dassie_hub::rules::Asset::Token {
                currency: dassie_xrpl::dsusd::CURRENCY,
                issuer: dassie_xrpl::dsusd::ISSUER,
            };
            terms.fees.gas = dassie_hub::rules::LEAST_TOKEN_GAS_DROPS;
            terms.fees.ledger = 0;
            terms.fees.least_payout = 1;
        });
        attest_all(&mut net, &ValidatorOperation::AttestEscrow { sequence: 5 });
        net.run(1_000);
        let member = NodeKey::from_bytes(&[31; 32]);
        let deposit = Deposit {
            transaction: Digest([1; 32]),
            ledger: 10,
            amount: 50_000_000,
            sender: [3; 20],
            tag: 0,
            key: Some(member.public_key()),
        };
        attest_all(&mut net, &ValidatorOperation::AttestDeposit(deposit));
        net.run(1_000);
        let ledger = net.drivers[0].validator().ledger();
        let (hub, number) = (
            *ledger.hub(),
            ledger.account_of(&member.public_key()).unwrap(),
        );
        let to = Destination {
            account: [9; 20],
            tag: None,
        };
        // The validators' signatures of the withdrawal reach no other
        // before the freeze.
        net.hold_shares = true;
        for (sequence, operation) in [
            (0, MemberOperation::SetDisbursal(to)),
            (
                1,
                MemberOperation::Withdraw {
                    amount: 5_000_000,
                    to,
                },
            ),
        ] {
            let signed =
                SignedMemberOperation::sign(&hub, number, sequence, 10, operation, &member);
            submit(&mut net, &Operation::Member(signed));
            net.run(1_000);
        }
        net
    }

    // spec/decisions/token-hubs.md: no escrow transaction goes to the XRPL
    // connection while the issuer holds a token hub's escrow, a
    // withdrawal whose signatures were made before the freeze included,
    // and it goes once the hub thaws.
    #[test]
    fn nothing_goes_to_a_frozen_escrow_until_the_thaw() {
        use dassie_hub::operation::Issuer;
        use dassie_xrpl::codec::{Payment, Transact};

        let mut net = token_hub_withdrawing();
        assert_eq!(net.drivers[0].validator().ledger().withdrawals().count(), 1);
        assert!(net.submits.is_empty(), "no quorum of signatures yet");

        // A quorum attests the freeze, and the signatures arrive during it.
        let issuer = |ledger, frozen| {
            ValidatorOperation::AttestIssuer(Issuer {
                ledger,
                frozen,
                fee: false,
            })
        };
        attest_all(&mut net, &issuer(20, true));
        net.run(1_000);
        assert!(
            net.drivers
                .iter()
                .all(|d| d.validator().ledger().issuer().holds())
        );
        net.hold_shares = false;
        let held = std::mem::take(&mut net.held);
        net.queue.extend(held);
        net.run(1_000);
        assert!(net.submits.is_empty(), "nothing submitted while frozen");

        // The thaw: each validator hands the connection the withdrawal.
        attest_all(&mut net, &issuer(21, false));
        net.run(1_000);
        assert!(
            net.drivers
                .iter()
                .all(|d| !d.validator().ledger().issuer().holds())
        );
        assert_eq!(net.submits.len(), 4, "{:?}", net.submits.len());
        for (_, blob) in &net.submits {
            assert!(Payment::decode(blob).unwrap().signatures_valid());
        }
    }

    /// The issuer's hold as of `ledger`: a freeze, or none.
    fn issuer(ledger: u32, frozen: bool) -> ValidatorOperation {
        ValidatorOperation::AttestIssuer(dassie_hub::operation::Issuer {
            ledger,
            frozen,
            fee: false,
        })
    }

    /// A validator's XRPL connection: its watcher of the escrow, on a fake
    /// XRP Ledger where the escrow is an account, and the transactions it
    /// submitted there.
    struct Connection {
        xrpl: dassie_xrpl::fake::FakeLedger,
        watcher: crate::hub::escrow::Watcher,
        link: u64,
        submitted: usize,
    }

    impl Connection {
        fn new(rules: &Rules) -> Self {
            use dassie_xrpl::fake::{FakeLedger, KeyedSignatures, LIST_SITE};
            const SECRET: [u8; 32] = *b"the driver's tests' validations.";
            let mut xrpl = FakeLedger::new();
            xrpl.sign_validations_with(SECRET);
            let escrow = dassie_xrpl::AccountId(rules.terms.escrow);
            xrpl.fund(escrow);
            xrpl.close();
            let unl = dassie_xrpl::validator_list::UnlConfig::for_network(rules.terms.network)
                .and_then(|own| own.with_override(&xrpl.publishers(LIST_SITE)))
                .unwrap();
            let watcher = crate::hub::escrow::Watcher::new(
                escrow,
                rules.terms.network,
                None,
                unl,
                Box::new(KeyedSignatures::new(SECRET)),
            );
            Self {
                xrpl,
                watcher,
                link: 0,
                submitted: 0,
            }
        }

        /// Carries out what the watcher asks, as the node's connection
        /// does, until it asks nothing more.
        fn run(&mut self, now: Millis, view: &EscrowView) {
            use crate::hub::escrow::Out;
            while let Some(out) = self.watcher.poll() {
                match out {
                    Out::Send(bytes) => {
                        if String::from_utf8_lossy(&bytes).contains("\"submit\"") {
                            self.submitted += 1;
                        }
                        let answer = self.xrpl.request(self.link, &bytes);
                        self.watcher.message(now, &answer, view);
                    }
                    Out::Fetch(url) => {
                        let body = self.xrpl.fetch(&url);
                        self.watcher.fetched(now, &url, body.as_deref());
                    }
                    _ => {}
                }
            }
        }

        fn connect(&mut self, now: Millis, view: &EscrowView) {
            self.link += 1;
            self.watcher.connected(now);
            self.run(now, view);
            self.watcher.tick(now, view);
            self.run(now, view);
        }
    }

    // An escrow transaction a quorum signed before the freeze, which the
    // XRPL connection queued while it was down, goes to the XRP Ledger
    // neither when the connection comes back during the freeze, nor later
    // while it lasts: the connection judges the hold when it sends, as of
    // the validator's last final block. It goes once the hub thaws.
    #[test]
    fn a_withdrawal_queued_before_a_freeze_waits_for_the_thaw() {
        let mut net = token_hub_withdrawing();
        net.hold_shares = false;
        let held = std::mem::take(&mut net.held);
        net.queue.extend(held);
        net.run(1_000);
        assert_eq!(net.submits.len(), 4, "signed before the freeze");
        let mut connection = Connection::new(&net.rules);
        let blob = net.submits.iter().find(|(from, _)| *from == 0).unwrap();
        connection.watcher.submit(blob.1.clone());

        attest_all(&mut net, &issuer(20, true));
        net.run(1_000);
        let view = net.drivers[0].escrow_view();
        assert!(view.issuer.holds());
        connection.connect(net.now, &view);
        assert!(connection.watcher.following());
        net.run(10_000);
        connection.watcher.tick(net.now, &view);
        connection.run(net.now, &view);
        assert_eq!(connection.submitted, 0, "nothing goes while frozen");

        attest_all(&mut net, &issuer(21, false));
        net.run(1_000);
        let view = net.drivers[0].escrow_view();
        assert!(!view.issuer.holds());
        connection.watcher.tick(net.now, &view);
        connection.run(net.now, &view);
        assert_eq!(connection.submitted, 1, "it goes on the thaw");
    }

    // The escrow's view names a deposit once the ledger recorded this
    // validator's attestation of it: one a full share refused during a
    // freeze is named only after the thaw, when the service attested it
    // again and the ledger recorded it.
    #[test]
    fn a_deposit_is_named_recorded_once_the_ledger_records_it() {
        use dassie_hub::digest::Digest;
        use dassie_hub::operation::Deposit;

        let mut net = Net::with_terms(|terms| {
            terms.asset = dassie_hub::rules::Asset::Token {
                currency: dassie_xrpl::dsusd::CURRENCY,
                issuer: dassie_xrpl::dsusd::ISSUER,
            };
            terms.fees.gas = dassie_hub::rules::LEAST_TOKEN_GAS_DROPS;
            terms.fees.ledger = 0;
            terms.limits.pending_deposits = 16;
        });
        attest_all(&mut net, &ValidatorOperation::AttestEscrow { sequence: 5 });
        net.run(1_000);
        attest_all(&mut net, &issuer(20, true));
        net.run(1_000);
        let share = net.drivers[0].escrow_view().deposit_room;
        assert_eq!(share, 4, "a quarter of the pending table");
        let deposit = |n: u8| Deposit {
            transaction: Digest([n; 32]),
            ledger: 30,
            amount: 50_000_000,
            sender: [3; 20],
            tag: 0,
            key: Some(NodeKey::from_bytes(&[n; 32]).public_key()),
        };
        for n in 1..=5 {
            attest_all(&mut net, &ValidatorOperation::AttestDeposit(deposit(n)));
            net.run(1_000);
        }
        let view = net.drivers[0].escrow_view();
        assert_eq!(view.deposit_room, 0, "the share is full while frozen");
        let named = |view: &EscrowView, n: u8| view.deposits_recorded.contains(&[n; 32]);
        assert!((1..=4).all(|n| named(&view, n)), "pending, with ours");
        assert!(!named(&view, 5), "refused for room");

        attest_all(&mut net, &issuer(21, false));
        net.run(2_000);
        let view = net.drivers[0].escrow_view();
        assert!(named(&view, 5), "attested again after the thaw");
        assert_eq!(net.drivers[0].validator().ledger().members(), 5);
    }

    // spec/decisions/ledger-resets.md: once the ledger wrote off a reset
    // test network's escrow, the old chain's deposits, which it records no
    // more, are waited for and named no more, so they never take the room
    // of the new chain's.
    #[test]
    fn a_reset_forgets_the_old_chains_deposits() {
        use dassie_hub::digest::Digest;
        use dassie_hub::operation::Deposit;

        let mut net = Net::with_terms(|terms| {
            terms.asset = dassie_hub::rules::Asset::Token {
                currency: dassie_xrpl::dsusd::CURRENCY,
                issuer: dassie_xrpl::dsusd::ISSUER,
            };
            terms.fees.gas = dassie_hub::rules::LEAST_TOKEN_GAS_DROPS;
            terms.fees.ledger = 0;
            terms.limits.pending_deposits = 16;
        });
        attest_all(&mut net, &ValidatorOperation::AttestEscrow { sequence: 5 });
        net.run(1_000);
        attest_all(&mut net, &issuer(20, true));
        net.run(1_000);
        for n in 1..=5 {
            let deposit = Deposit {
                transaction: Digest([n; 32]),
                ledger: 30,
                amount: 50_000_000,
                sender: [3; 20],
                tag: 0,
                key: Some(NodeKey::from_bytes(&[n; 32]).public_key()),
            };
            attest_all(&mut net, &ValidatorOperation::AttestDeposit(deposit));
            net.run(1_000);
        }
        assert_eq!(net.drivers[0].awaited.len(), 1, "one refused for room");
        assert!(!net.drivers[0].recorded.is_empty());

        attest_all(&mut net, &ValidatorOperation::AttestReset { ledger: 1 });
        net.run(1_000);
        for driver in &net.drivers {
            assert!(driver.validator().ledger().written_off_at().is_some());
            assert!(driver.awaited.is_empty(), "the old chain's waits no more");
            assert!(driver.escrow_view().deposits_recorded.is_empty());
        }
    }

    // spec/areas/hubs.md#payout-bundles: a hub that stalled while the
    // issuer held it makes no block that could show the thaw. Each
    // validator signs its payout set once its own watcher proved the thaw,
    // and not while what it proved still holds the escrow.
    #[test]
    fn a_hub_stopped_while_held_pays_out_once_the_thaw_is_proven() {
        let mut net = token_hub_withdrawing();
        attest_all(&mut net, &issuer(20, true));
        net.run(1_000);

        // The validators are cut off from each other for longer than a
        // stall: each declares it, and signs no set while held.
        net.cut = true;
        net.now += net.rules.terms.timers.stall + 1_000;
        for i in 0..net.drivers.len() {
            let step = net.drivers[i].tick(net.now).unwrap();
            net.deliver(i, step);
        }
        assert!(
            net.drivers
                .iter()
                .all(|d| d.validator().engine().is_stalled())
        );
        assert!(net.bundles.is_empty(), "no set while held");

        // A freeze proven later still holds it; the thaw lets it go.
        for (ledger, frozen) in [(30, true), (31, false)] {
            for i in 0..net.drivers.len() {
                let (step, _) = net.drivers[i]
                    .attest(net.now, issuer(ledger, frozen))
                    .unwrap();
                net.deliver(i, step);
            }
            assert_eq!(net.bundles.is_empty(), frozen, "{ledger}");
        }
        assert_eq!(net.bundles.len(), 4, "each validator signs its set");
    }
}
