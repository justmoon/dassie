//! One hub's validator in the node, without its IO: a
//! `dassie_hub::service::Validator`, what it persisted, and what each step
//! asks the node to do.
//!
//! Every input is a step: a message, the time, or something seen on the
//! escrow. After it, the step's consensus batches are folded into the
//! stored state and written, with the ledger after a block that became
//! final, in one transaction, and only then are its actions handed out
//! ([`Effects`]): a validator never sends a vote it could forget in a
//! crash, and comes back from the state after its last final block
//! (spec/areas/hubs.md#consensus). A write that fails ends the step with
//! nothing handed out.

use std::sync::Arc;

use dassie_hub::consensus::{Finalized, PeerId, Stored};
use dassie_hub::escrow::{PayoutSet, ValidatorKeys};
use dassie_hub::ledger::Ledger;
use dassie_hub::operation::ValidatorOperation;
use dassie_hub::rules::Rules;
use dassie_hub::service::payload::Payload;
use dassie_hub::service::wire::Message;
use dassie_hub::service::{Action, CatchUpError, Validator, genesis_block};
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
        let keys = ValidatorKeys::derive(node_key, &rules.terms.nonce)
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
            return Ok(Self {
                hub,
                own,
                validator,
                stored,
                written_height: saved.height,
                caught_up: 0,
                store,
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
        })
    }

    /// The hub.
    #[must_use]
    pub const fn hub(&self) -> &HubId {
        &self.hub
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
    /// The step could not be written: nothing of it goes out.
    pub fn handle(
        &mut self,
        now: Millis,
        from: PeerId,
        node: Option<&PublicKey>,
        message: &[u8],
    ) -> Result<Effects, StoreError> {
        self.validator.handle(now, from, node, message);
        self.finish(now, Some(from))
    }

    /// Takes the state after `finalized`, fetched from a validator: only
    /// if it is the state the block names. The new state is written as any
    /// step's.
    ///
    /// # Errors
    ///
    /// The state was not taken, and nothing changed; or the step could not
    /// be written, and nothing of it goes out.
    pub fn catch_up(
        &mut self,
        now: Millis,
        finalized: Finalized,
        snapshot: &[u8],
    ) -> Result<Result<Effects, CatchUpError>, StoreError> {
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
    /// The step could not be written: nothing of it goes out.
    pub fn tick(&mut self, now: Millis) -> Result<Effects, StoreError> {
        self.validator.tick(now);
        self.finish(now, None)
    }

    /// Attests what the node saw of the escrow, and whether the validator
    /// took it: one its pool had no room for is attested again later.
    ///
    /// # Errors
    ///
    /// The step could not be written: nothing of it goes out.
    pub fn attest(
        &mut self,
        now: Millis,
        operation: ValidatorOperation,
    ) -> Result<(Effects, bool), StoreError> {
        let taken = self.validator.attest(now, operation);
        Ok((self.finish(now, None)?, taken))
    }

    /// Persists the step, then hands out its actions.
    fn finish(&mut self, now: Millis, from: Option<PeerId>) -> Result<Effects, StoreError> {
        self.persist(now)?;
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
                    self.keep_bundle(now, &set, &signatures)?;
                    effects.bundles.push(set.height);
                }
                Action::Behind(finalized) => effects.behind = Some(finalized),
            }
        }
        Ok(effects)
    }

    /// Folds the step's batches into the stored state and writes it, with
    /// the ledger if a block became final.
    fn persist(&mut self, now: Millis) -> Result<(), StoreError> {
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
            return Ok(());
        }
        if !self.stored.fits() {
            return Err(StoreError::Corrupt("hub consensus state over its bounds"));
        }
        let consensus = changed.then(|| self.stored.to_vec());
        let snapshot = (height != self.written_height).then(|| ledger.snapshot());
        let step = HubStep {
            consensus: consensus.as_deref(),
            ledger: snapshot.as_deref().map(|bytes| (height, bytes)),
            boundaries: &boundaries,
        };
        self.store.save_hub_step(self.hub.0.as_bytes(), now, step)?;
        self.written_height = height;
        Ok(())
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

    /// Keeps our signatures of a payout set, retrievable as the hub's
    /// `Share` messages in sequence order (spec/designs/settlement-hubs.md#9-shutdown).
    fn keep_bundle(
        &self,
        now: Millis,
        set: &PayoutSet,
        signatures: &[Signer],
    ) -> Result<(), StoreError> {
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
        let bundle = encode_frames(&shares);
        self.store
            .add_hub_bundle(self.hub.0.as_bytes(), set.height, now, &bundle)
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
    }

    impl Net {
        fn new() -> Self {
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
            let rules = rules(&NodeKey::from_bytes(&[100; 32]), &endpoints);
            let dirs: Vec<tempfile::TempDir> =
                (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
            let mut net = Self {
                dirs,
                nodes,
                rules,
                drivers: Vec::new(),
                queue: VecDeque::new(),
                now: START,
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

        fn deliver(&mut self, from: usize, effects: Effects) {
            for (peer, message) in effects.sends {
                let to = self.index_of(&peer.node);
                self.queue.push_back((to, from, message));
            }
        }

        fn run(&mut self, ms: u64) {
            for _ in 0..ms {
                self.now += 1;
                for i in 0..self.drivers.len() {
                    if self.drivers[i].deadline().is_some_and(|at| at <= self.now) {
                        let effects = self.drivers[i].tick(self.now).unwrap();
                        self.deliver(i, effects);
                    }
                }
                while let Some((to, from, message)) = self.queue.pop_front() {
                    let peer = PeerId(u64::try_from(from).unwrap());
                    let node = self.nodes[from].public_key();
                    let effects = self.drivers[to]
                        .handle(self.now, peer, Some(&node), &message)
                        .unwrap();
                    for reply in &effects.reply {
                        self.queue.push_back((from, to, reply.clone()));
                    }
                    self.deliver(to, effects);
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
            let (effects, taken) = net.drivers[i].attest(net.now, attest).unwrap();
            assert!(taken);
            net.deliver(i, effects);
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
}
