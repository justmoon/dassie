//! The validator role (spec/areas/hubs.md#the-validator-in-the-node): a
//! supervised service that runs a validator for each hub the owner
//! configured, once the node has its key.
//!
//! Each hub has one task that owns its [`Driver`] and takes, one at a time,
//! the messages the hub's stream brings, the time, and what the node sees
//! of the escrow. It hands each step to the writer the hubs share
//! ([`HubWriter`]), which commits the steps of every hub waiting then
//! together. After each step is on disk it answers the message's
//! sender, sends to the other validators over QUIC, each an exchange of its
//! own whose reply comes back as a message from that validator, and hands
//! the escrow's transactions to its XRPL connection ([`super::xrpl`]). A
//! write that fails stops the service, which the supervisor starts again
//! from what is on disk.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dassie_hub::HubId;
use dassie_hub::consensus::Finalized;
use dassie_hub::operation::ValidatorOperation;
use dassie_identity::{IdentityKey, NodeId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::config::{HubConfig, ValidatorConfig};
use super::driver::{Driver, DriverError, Effects, Peer, Step, hex};
use super::escrow::EscrowView;
use super::transport::{Hubs, INBOX, Inbound, Route, Sender, bounded};
use super::writer::HubWriter;
use super::{HubAnswer, HubClient, HubExchangeError, xrpl};
use crate::clock::{Clock, clock_moved};
use crate::log::target;
use crate::node::Node;
use crate::store::StoreError;

/// The longest an exchange with another validator takes: a consensus
/// message is worth little later, and is sent again if it matters.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// The most exchanges with other validators in flight for one hub: past
/// it, a message is dropped, as a network drops one.
pub const MAX_SENDS_IN_FLIGHT: usize = 256;

/// The longest a validator waits between two ticks when its engine names
/// no deadline: it still notices a stall.
const IDLE_TICK_MS: u64 = 1_000;

/// The most attestations and escrow transactions waiting between a hub's
/// validator and its XRPL connection, each way.
const ESCROW_QUEUE: usize = 64;

/// The first wait before sending to a validator that could not be reached.
const BACKOFF_MIN: Duration = Duration::from_millis(250);

/// The longest wait before sending to a validator that could not be
/// reached: one that comes back is sent to again within it.
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How long a validator that answered busy is left alone: it is up, and
/// its places for validators' requests were taken for a moment, as in a
/// burst of a node that validates several hubs. Consensus sends again what
/// matters.
const BUSY_RETRY: Duration = Duration::from_millis(50);

/// Runs the validators of the hubs in `config`, and of those founded with
/// the node (spec/areas/hubs.md#founding-a-hub), as each is founded, until
/// `shutdown`, once the node has its key: at most
/// [`MAX_VALIDATED_HUBS`](super::config::MAX_VALIDATED_HUBS). A hub the node
/// is not a validator of, or whose stored state is broken, is left out,
/// and said so.
pub async fn run(node: Arc<Node>, config: Arc<ValidatorConfig>, shutdown: CancellationToken) {
    let Some(key) = identity(&node, &shutdown).await else {
        return;
    };
    let client = HubClient::of(&node);
    let shared = Shared::new(&node);
    let tasks = TaskTracker::new();
    let stop = shutdown.child_token();
    let mut founded = node.founding().map(|founding| founding.founded());
    let mut seen: BTreeSet<HubId> = BTreeSet::new();
    let mut running: Vec<HubId> = Vec::new();
    while !stop.is_cancelled() {
        if let Some(founded) = &mut founded {
            founded.mark_unchanged();
        }
        for hub in hubs(&node, &config) {
            let id = hub.rules.id();
            if !seen.insert(id) {
                continue;
            }
            if running.len() >= super::config::MAX_VALIDATED_HUBS {
                tracing::warn!(target: target::HUB, hub = %hex(id.0.as_bytes()), "validating as many hubs as the node does: not this one");
                continue;
            }
            match start(
                &node,
                &key,
                (&hub, &config),
                (&client, &shared),
                &tasks,
                &stop,
            ) {
                Ok(true) => running.push(id),
                Ok(false) => {}
                // The database failed: the service starts again.
                Err(()) => stop.cancel(),
            }
        }
        let changed = async {
            match &mut founded {
                Some(founded) => {
                    if founded.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = stop.cancelled() => {}
            () = changed => {}
        }
    }
    tasks.close();
    tasks.wait().await;
    for hub in &running {
        node.hubs().unregister(hub);
    }
}

/// The hubs to validate: the owner's file's, then those founded with the
/// node.
fn hubs(node: &Node, config: &ValidatorConfig) -> Vec<HubConfig> {
    let mut hubs = config.hubs.clone();
    if let Some(founding) = node.founding() {
        match founding.hubs(&node.store_handle()) {
            Ok(founded) => hubs.extend(founded),
            Err(error) => {
                tracing::error!(target: target::HUB, %error, "cannot read the hubs founded with the node");
            }
        }
    }
    hubs
}

/// The node's key, once it has one; `None` at shutdown.
async fn identity(node: &Node, shutdown: &CancellationToken) -> Option<Arc<IdentityKey>> {
    let mut identity = node.identity();
    tokio::select! {
        () = shutdown.cancelled() => None,
        key = identity.wait_for(Option::is_some) => key.ok().and_then(|key| key.clone()),
    }
}

/// Opens the validator of `hub` and starts its tasks, which cancel `stop`
/// if its store fails: whether it started, or `Err` if the database failed.
fn start(
    node: &Arc<Node>,
    key: &IdentityKey,
    (hub, config): (&HubConfig, &ValidatorConfig),
    (client, shared): (&HubClient, &Shared),
    tasks: &TaskTracker,
    stop: &CancellationToken,
) -> Result<bool, ()> {
    let unl = &config.unl;
    let id = hub.rules.id();
    let opened = Driver::open(
        node.store_handle(),
        &hub.rules,
        hub.genesis_ms,
        key,
        node.now_ms(),
    );
    let driver = match opened {
        Ok(driver) => driver,
        Err(DriverError::Store(error)) => {
            tracing::error!(target: target::HUB, hub = %hex(id.0.as_bytes()), %error, "cannot open the hub's validator");
            return Err(());
        }
        Err(error) => {
            tracing::error!(target: target::HUB, hub = %hex(id.0.as_bytes()), %error, "not validating the hub");
            return Ok(false);
        }
    };
    tracing::info!(target: target::HUB, hub = %hex(id.0.as_bytes()), height = driver.validator().ledger().height(), "validating the hub");
    let (task, links) = HubTask::new(
        driver,
        client.clone(),
        Arc::clone(node.hubs()),
        node.clock(),
    );
    let task = task.voting_for(config.hub_version).sharing(shared);
    let follow = xrpl::Follow {
        store: node.store_handle(),
        hub: id,
        url: hub.xrpl.clone(),
        client: node.outbound().cloned(),
        clock: node.clock(),
        links,
        retry: std::collections::VecDeque::new(),
        unl: unl.clone(),
    };
    let stop_ = stop.clone();
    tasks.spawn(async move {
        if let Err(error) = task.run(stop_.clone()).await {
            tracing::error!(target: target::HUB, %error, "a hub's validator could not write its step; starting again");
            stop_.cancel();
        }
    });
    let stop_ = stop.clone();
    tasks.spawn(async move { follow.run(stop_).await });
    Ok(true)
}

/// What the validators of the node's hubs share, for as long as the
/// service runs: the writer of their steps.
#[derive(Clone, Debug)]
pub struct Shared {
    writer: HubWriter,
}

impl Shared {
    /// What the validators of `node`'s hubs share.
    #[must_use]
    pub fn new(node: &Node) -> Self {
        Self::of(node.store_handle(), node.hubs())
    }

    /// What validators writing to `store` share, counted in `hubs`.
    #[must_use]
    pub fn of(store: Arc<crate::store::Store>, hubs: &Hubs) -> Self {
        Self {
            writer: HubWriter::new(store, Arc::clone(hubs.write_counters())),
        }
    }
}

/// What a hub's validator and its XRPL connection share.
#[derive(Debug)]
pub struct EscrowLinks {
    /// What the connection saw, for the validator to attest.
    pub attest: mpsc::Sender<ValidatorOperation>,
    /// Escrow transactions a quorum signed, for the connection to submit.
    pub submits: mpsc::Receiver<Vec<u8>>,
    /// What the ledger says that bears on the escrow.
    pub view: watch::Receiver<EscrowView>,
    /// Whether the connection follows the escrow now.
    pub connected: watch::Sender<bool>,
    /// Woken when a founding asks whether a quorum checked the escrow.
    pub check: Arc<tokio::sync::Notify>,
}

/// The longest fetching the state after a final block from one validator
/// takes: a state is far larger than a message.
pub const STATE_TIMEOUT: Duration = Duration::from_secs(60);

/// The least time between two fetches of a state: anyone can name a block
/// far ahead, which is unverified until its state is loaded
/// (spec/areas/hubs.md#consensus), so the host limits how often it loads
/// one.
pub const CATCH_UP_EVERY: Duration = Duration::from_secs(1);

/// The most attestations waiting for room in the validator's own pool.
const MAX_RETRIED: usize = 64;

/// A state fetched from a validator, for the block it is after.
type Fetched = (Box<Finalized>, Vec<u8>);

/// What a hub's task takes, one at a time.
// One input at a time: boxing new rules, which are rare, would buy nothing.
#[allow(clippy::large_enum_variant)]
enum Input {
    Tick,
    Fetched(Box<Finalized>, Vec<u8>),
    Message(Inbound),
    Attest(ValidatorOperation),
}

/// When each validator that could not be reached is sent to again.
type Backoffs = Arc<Mutex<BTreeMap<NodeId, (tokio::time::Instant, Duration)>>>;

/// One hub's validator at work.
pub struct HubTask {
    driver: Driver,
    /// What the hub's other validators send, which members cannot crowd
    /// out: taken first.
    from_validators: mpsc::Receiver<Inbound>,
    /// What anyone else sends.
    from_members: mpsc::Receiver<Inbound>,
    /// Where replies of the other validators go, as messages from them.
    loopback: mpsc::Sender<Inbound>,
    /// The other validators' nodes, as the route last had them.
    validator_nodes: BTreeSet<NodeId>,
    client: HubClient,
    hubs: Arc<Hubs>,
    clock: Arc<dyn Clock>,
    /// What writes its steps.
    writer: HubWriter,
    attestations: mpsc::Receiver<ValidatorOperation>,
    submits: mpsc::Sender<Vec<u8>>,
    view: watch::Sender<EscrowView>,
    xrpl_connected: watch::Receiver<bool>,
    permits: Arc<Semaphore>,
    backoffs: Backoffs,
    sends: TaskTracker,
    /// States fetched for catching up, and where fetches send them.
    fetched: mpsc::Receiver<Fetched>,
    fetches: mpsc::Sender<Fetched>,
    /// When the last fetch started, if one did; one runs at a time.
    fetching: Arc<std::sync::atomic::AtomicBool>,
    last_fetch: Option<tokio::time::Instant>,
    /// Attestations the validator's pool had no room for, to attest again.
    retried: std::collections::VecDeque<ValidatorOperation>,
    /// Escrow transactions the XRPL connection had no room for yet, in
    /// order: at most [`ESCROW_QUEUE`].
    unsent: std::collections::VecDeque<Vec<u8>>,
    /// The version of the hub's protocol the owner has us vote to move the
    /// hub to (spec/decisions/hub-protocol-version.md).
    hub_version: Option<u16>,
}

impl std::fmt::Debug for HubTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubTask")
            .field("driver", &self.driver)
            .finish_non_exhaustive()
    }
}

impl HubTask {
    /// The task of `driver`'s validator, which takes what `hubs` routes to
    /// its hub, and the links its XRPL connection takes.
    #[must_use]
    pub fn new(
        driver: Driver,
        client: HubClient,
        hubs: Arc<Hubs>,
        clock: Arc<dyn Clock>,
    ) -> (Self, EscrowLinks) {
        let (loopback, from_validators) = mpsc::channel(INBOX);
        let (members, from_members) = mpsc::channel(INBOX);
        let validator_nodes = driver.peer_nodes();
        let route = Route {
            validators: loopback.clone(),
            members,
            nodes: validator_nodes.clone(),
        };
        hubs.register(*driver.hub(), route);
        let check = Arc::new(tokio::sync::Notify::new());
        hubs.register_escrow_check(*driver.hub(), Arc::clone(&check));
        let (attest, attestations) = mpsc::channel(ESCROW_QUEUE);
        let (submits, submitted) = mpsc::channel(ESCROW_QUEUE);
        let (view, viewed) = watch::channel(driver.escrow_view());
        let (connected, xrpl_connected) = watch::channel(false);
        let (fetches, fetched) = mpsc::channel(1);
        hubs.set_status(driver.status(false));
        let writer = Shared::of(Arc::clone(driver.store()), &hubs).writer;
        let task = Self {
            driver,
            from_validators,
            from_members,
            loopback,
            validator_nodes,
            client,
            hubs,
            clock,
            writer,
            attestations,
            submits,
            view,
            xrpl_connected,
            permits: Arc::new(Semaphore::new(MAX_SENDS_IN_FLIGHT)),
            backoffs: Arc::default(),
            sends: TaskTracker::new(),
            fetched,
            fetches,
            fetching: Arc::default(),
            last_fetch: None,
            retried: std::collections::VecDeque::new(),
            unsent: std::collections::VecDeque::new(),
            hub_version: None,
        };
        let links = EscrowLinks {
            attest,
            submits: submitted,
            view: viewed,
            connected,
            check,
        };
        (task, links)
    }

    /// The task voting to move the hub to `version` of the hub's protocol
    /// while it runs an older one, as the owner's file says; none votes for
    /// nothing.
    #[must_use]
    pub const fn voting_for(mut self, version: Option<u16>) -> Self {
        self.hub_version = version;
        self
    }

    /// The task sharing what the validators of the node's other hubs
    /// share: its steps are written with theirs. Without it, a task writes
    /// alone.
    #[must_use]
    pub fn sharing(mut self, shared: &Shared) -> Self {
        self.writer = shared.writer.clone();
        self
    }

    /// Runs until `shutdown`.
    ///
    /// # Errors
    ///
    /// A step could not be written: nothing of it went out.
    pub async fn run(mut self, shutdown: CancellationToken) -> Result<(), StoreError> {
        let result = self.steps(&shutdown).await;
        self.hubs.unregister(self.driver.hub());
        self.sends.close();
        self.sends.wait().await;
        result
    }

    async fn steps(&mut self, shutdown: &CancellationToken) -> Result<(), StoreError> {
        let mut moves = self.clock.subscribe();
        loop {
            let now = self.clock.now_ms();
            let wake = self
                .driver
                .deadline()
                .unwrap_or(u64::MAX)
                .min(now.saturating_add(IDLE_TICK_MS));
            let sleep = Duration::from_millis(wake.saturating_sub(now));
            // In this order: the time, the other validators, what the
            // escrow showed, and only then members, who cannot hold up
            // consensus however much they send.
            match tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(()),
                () = tokio::time::sleep(sleep) => Input::Tick,
                () = clock_moved(&mut moves) => Input::Tick,
                Some((finalized, state)) = self.fetched.recv() => Input::Fetched(finalized, state),
                Some(inbound) = self.from_validators.recv() => Input::Message(inbound),
                Some(operation) = self.attestations.recv() => Input::Attest(operation),
                Some(inbound) = self.from_members.recv() => Input::Message(inbound),
            } {
                Input::Tick => {
                    let step = self.driver.tick(self.clock.now_ms())?;
                    self.carry(step, None, shutdown).await?;
                    // Attests again one of what found no room.
                    if let Some(operation) = self.retried.pop_front() {
                        self.attest(operation, shutdown).await?;
                    }
                    // Votes to move the hub to the version the owner names,
                    // until the hub runs it.
                    if let Some(version) = self.hub_version
                        && self.driver.validator().wants_upgrade(version)
                    {
                        tracing::info!(target: target::HUB, hub = %hex(self.driver.hub().0.as_bytes()), version, "voting to move the hub to a newer version of its protocol");
                        self.attest(ValidatorOperation::Upgrade { version }, shutdown)
                            .await?;
                    }
                }
                Input::Fetched(finalized, state) => {
                    self.catch_up(*finalized, &state, shutdown).await?;
                }
                Input::Message(inbound) => {
                    heard_from(&self.backoffs, &inbound);
                    let now = self.clock.now_ms();
                    let node = inbound.node.as_ref();
                    let step = self
                        .driver
                        .handle(now, inbound.from, node, &inbound.message)?;
                    self.carry(step, inbound.reply, shutdown).await?;
                }
                Input::Attest(operation) => self.attest(operation, shutdown).await?,
            }
            self.hand_over();
            self.publish();
        }
    }

    /// Hands the XRPL connection the escrow transactions waiting for it, in
    /// order, while it has room.
    fn hand_over(&mut self) {
        while let Some(blob) = self.unsent.pop_front() {
            if let Err(error) = self.submits.try_send(blob) {
                if let mpsc::error::TrySendError::Full(blob) = error {
                    self.unsent.push_front(blob);
                }
                return;
            }
        }
    }

    /// Writes a step, with the steps of the node's other hubs waiting then,
    /// and only then answers whoever sent its message and carries out its
    /// actions.
    async fn carry(
        &mut self,
        step: Step,
        reply: Option<tokio::sync::oneshot::Sender<Vec<Vec<u8>>>>,
        shutdown: &CancellationToken,
    ) -> Result<(), StoreError> {
        if !step.write.is_empty() {
            self.writer.write(step.write).await?;
        }
        if let Some(reply) = reply {
            let _ = reply.send(bounded(step.effects.reply.clone()));
        }
        self.carry_out(step.effects, shutdown);
        Ok(())
    }

    /// Attests `operation`, or keeps it to attest again later if the
    /// validator's pool has no room for it now.
    async fn attest(
        &mut self,
        operation: ValidatorOperation,
        shutdown: &CancellationToken,
    ) -> Result<(), StoreError> {
        let (step, taken) = self.driver.attest(self.clock.now_ms(), operation.clone())?;
        if !taken && self.retried.len() < MAX_RETRIED {
            self.retried.push_back(operation);
        }
        self.carry(step, None, shutdown).await
    }

    /// Takes the state a validator sent for catching up.
    async fn catch_up(
        &mut self,
        finalized: Finalized,
        state: &[u8],
        shutdown: &CancellationToken,
    ) -> Result<(), StoreError> {
        let height = finalized.block.height;
        match self
            .driver
            .catch_up(self.clock.now_ms(), finalized, state)?
        {
            Ok(step) => {
                tracing::info!(target: target::HUB, hub = %hex(self.driver.hub().0.as_bytes()), height, "caught up from a validator's state");
                self.carry(step, None, shutdown).await
            }
            Err(error) => {
                tracing::warn!(target: target::HUB, height, ?error, "a validator's state was not the one its block names");
                Ok(())
            }
        }
    }

    /// Tells the node how the validator stands after a step: its status,
    /// its co-validators' nodes, and what the escrow's watcher needs.
    fn publish(&mut self) {
        let connected = *self.xrpl_connected.borrow();
        self.hubs.set_status(self.driver.status(connected));
        let nodes = self.driver.peer_nodes();
        if nodes != self.validator_nodes {
            self.hubs.set_validators(self.driver.hub(), nodes.clone());
            self.validator_nodes = nodes;
        }
        let view = self.driver.escrow_view();
        self.view.send_if_modified(|seen| {
            let changed = *seen != view;
            *seen = view;
            changed
        });
    }

    fn carry_out(&mut self, effects: Effects, shutdown: &CancellationToken) {
        for (peer, message) in effects.sends {
            self.send(peer, message, shutdown);
        }
        if let Some(finalized) = effects.behind {
            self.fetch(finalized, shutdown);
        }
        // Each goes once the connection has room; past the bound the newest
        // is dropped. The escrow's sequences apply in order, so only those
        // signed after it could wait on it, never one that came before.
        for blob in effects.submits {
            if self.unsent.len() >= ESCROW_QUEUE {
                tracing::warn!(target: target::HUB, "the XRPL connection is not keeping up: an escrow transaction was dropped");
                continue;
            }
            self.unsent.push_back(blob);
        }
        self.hand_over();
    }

    /// Sends `message` to `peer`, unless it could not be reached lately or
    /// too many exchanges are in flight; its reply comes back as messages
    /// from it.
    fn send(&self, peer: Peer, message: Vec<u8>, shutdown: &CancellationToken) {
        if backing_off(&self.backoffs, &peer.node) {
            return;
        }
        let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            return;
        };
        let exchange = Exchange {
            client: self.client.clone(),
            hub: *self.driver.hub(),
            loopback: self.loopback.clone(),
            backoffs: Arc::clone(&self.backoffs),
            _permit: permit,
        };
        let shutdown = shutdown.clone();
        self.sends.spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = exchange.run(peer, message) => {}
            }
        });
    }
}

impl HubTask {
    /// Fetches the state after `finalized` from the validators, one after
    /// another until one has it: one fetch at a time, and at most one per
    /// [`CATCH_UP_EVERY`].
    fn fetch(&mut self, finalized: Box<Finalized>, shutdown: &CancellationToken) {
        use std::sync::atomic::Ordering;
        let due = self
            .last_fetch
            .is_none_or(|at| at.elapsed() >= CATCH_UP_EVERY);
        if !due || self.fetching.swap(true, Ordering::AcqRel) {
            return;
        }
        self.last_fetch = Some(tokio::time::Instant::now());
        let peers = self.driver.peers();
        let (client, hub) = (self.client.clone(), *self.driver.hub());
        let (fetches, fetching) = (self.fetches.clone(), Arc::clone(&self.fetching));
        let shutdown = shutdown.clone();
        self.sends.spawn(async move {
            let height = finalized.block.height;
            for peer in peers {
                let fetched = tokio::select! {
                    () = shutdown.cancelled() => break,
                    fetched = client.fetch_state(peer.node, &peer.endpoint, &hub, height, STATE_TIMEOUT) => fetched,
                };
                match fetched {
                    Ok(state) => {
                        let _ = fetches.send((finalized, state)).await;
                        break;
                    }
                    Err(error) => {
                        tracing::debug!(target: target::HUB, node = %peer.node, height, %error, "no state from a validator");
                    }
                }
            }
            fetching.store(false, Ordering::Release);
        });
    }
}

/// One exchange with another validator.
struct Exchange {
    client: HubClient,
    hub: HubId,
    loopback: mpsc::Sender<Inbound>,
    backoffs: Backoffs,
    _permit: OwnedSemaphorePermit,
}

impl Exchange {
    async fn run(self, peer: Peer, message: Vec<u8>) {
        let sent = self
            .client
            .exchange(peer.node, &peer.endpoint, &self.hub, &message, SEND_TIMEOUT)
            .await;
        let mut backoffs = self.backoffs.lock().unwrap_or_else(PoisonError::into_inner);
        match sent {
            Ok(messages) => {
                backoffs.remove(&peer.node);
                drop(backoffs);
                let from = Sender(peer.key).peer();
                for message in messages {
                    let inbound = Inbound {
                        from,
                        node: Some(peer.key),
                        message,
                        reply: None,
                    };
                    let _ = self.loopback.try_send(inbound);
                }
            }
            Err(error) => {
                failed(&mut backoffs, peer.node, &error);
                tracing::debug!(target: target::HUB, node = %peer.node, %error, "a validator could not be reached");
            }
        }
    }
}

/// How long to leave a validator alone after an exchange failed with
/// `error`, and the wait the next failure doubles from, after `previous`:
/// one that answered busy is tried again shortly, and its wait does not
/// grow; one that could not be reached waits twice as long as last time,
/// from [`BACKOFF_MIN`] up to [`BACKOFF_MAX`].
fn after_failure(error: &HubExchangeError, previous: Option<Duration>) -> (Duration, Duration) {
    if matches!(error, HubExchangeError::Refused(HubAnswer::Busy)) {
        return (BUSY_RETRY, previous.unwrap_or(Duration::ZERO));
    }
    let wait = previous
        .filter(|wait| !wait.is_zero())
        .map_or(BACKOFF_MIN, |wait| (wait * 2).min(BACKOFF_MAX));
    (wait, wait)
}

/// Leaves `node` alone for a while after an exchange with it failed with
/// `error`.
fn failed(
    backoffs: &mut BTreeMap<NodeId, (tokio::time::Instant, Duration)>,
    node: NodeId,
    error: &HubExchangeError,
) {
    let previous = backoffs.get(&node).map(|(_, wait)| *wait);
    let (pause, wait) = after_failure(error, previous);
    backoffs.insert(node, (tokio::time::Instant::now() + pause, wait));
}

/// Ends the wait before sending again to the node that sent `inbound`, if
/// the transport proved its key: a validator that comes back is sent to as
/// soon as anything of it arrives (spec/areas/hubs.md#the-validator-in-the-node).
/// Only the proved key counts, never the handle a message names, so nobody
/// ends the wait for a node but that node itself, and all it can make us
/// do is send to it, each exchange bounded by [`SEND_TIMEOUT`], which it
/// can anyway by answering slowly. The wait the next failure doubles from
/// stays: one that reaches us but that we cannot reach still waits longer
/// after each failure, until an exchange with it succeeds. It adds no
/// entry.
fn heard_from(backoffs: &Backoffs, inbound: &Inbound) {
    let Some(key) = inbound.node else {
        return;
    };
    let mut held = backoffs.lock().unwrap_or_else(PoisonError::into_inner);
    if held.is_empty() {
        return;
    }
    if let Some((until, _)) = held.get_mut(&key.node_id()) {
        *until = (*until).min(tokio::time::Instant::now());
    }
}

/// Whether `node` could not be reached lately, so nothing is sent to it
/// until its wait is over.
fn backing_off(backoffs: &Backoffs, node: &NodeId) -> bool {
    backoffs
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(node)
        .is_some_and(|(until, _)| *until > tokio::time::Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dassie_identity::NodeIdentity;

    // A validator that answered busy is up: it is tried again shortly, and
    // a burst of busy answers never backs off from it for longer.
    #[test]
    fn busy_is_no_failure_to_back_off_from() {
        let busy = HubExchangeError::Refused(HubAnswer::Busy);
        let unreachable = HubExchangeError::Unreachable;
        let mut wait = None;
        for _ in 0..10 {
            let (pause, next) = after_failure(&busy, wait);
            assert_eq!(pause, BUSY_RETRY);
            wait = Some(next);
        }
        assert_eq!(
            after_failure(&unreachable, wait),
            (BACKOFF_MIN, BACKOFF_MIN)
        );
        let mut wait = None;
        for _ in 0..10 {
            wait = Some(after_failure(&unreachable, wait).1);
        }
        assert_eq!(
            wait,
            Some(BACKOFF_MAX),
            "an unreachable one is left alone longer"
        );
        assert_eq!(after_failure(&busy, wait), (BUSY_RETRY, BACKOFF_MAX));
    }

    fn from(sender: NodeIdentity, proved: Option<NodeIdentity>) -> Inbound {
        Inbound {
            from: Sender(sender).peer(),
            node: proved,
            message: Vec::new(),
            reply: None,
        }
    }

    // A validator backed off from a peer it could not reach sends to it again
    // as soon as the peer is heard from, by the key the transport proved;
    // nothing else ends the wait, and the wait still grows with the next
    // failure.
    #[test]
    fn hearing_from_a_validator_ends_the_wait_for_it_alone() {
        let peer = dassie_identity::TableKeygen::identity_key(&[1; 32]).identity();
        let other = dassie_identity::TableKeygen::identity_key(&[2; 32]).identity();
        let backoffs = Backoffs::default();
        {
            let mut held = backoffs.lock().unwrap();
            for _ in 0..10 {
                failed(&mut held, peer.node_id(), &HubExchangeError::Unreachable);
            }
            failed(&mut held, other.node_id(), &HubExchangeError::Unreachable);
        }
        assert!(backing_off(&backoffs, &peer.node_id()));

        // Another node, one that names the peer's handle, and one whose key
        // nothing proved, end nothing.
        heard_from(&backoffs, &from(other, Some(other)));
        heard_from(&backoffs, &from(peer, Some(other)));
        heard_from(&backoffs, &from(peer, None));
        assert!(
            backing_off(&backoffs, &peer.node_id()),
            "only the peer itself ends its wait"
        );
        assert!(!backing_off(&backoffs, &other.node_id()));

        // The peer itself does, and the other's wait is untouched by it.
        failed(
            &mut backoffs.lock().unwrap(),
            other.node_id(),
            &HubExchangeError::Unreachable,
        );
        heard_from(&backoffs, &from(peer, Some(peer)));
        assert!(!backing_off(&backoffs, &peer.node_id()), "sent to at once");
        assert!(backing_off(&backoffs, &other.node_id()));

        // Its wait still grows from where it was.
        let mut held = backoffs.lock().unwrap();
        failed(&mut held, peer.node_id(), &HubExchangeError::Unreachable);
        assert_eq!(
            held.get(&peer.node_id()).map(|(_, wait)| *wait),
            Some(BACKOFF_MAX)
        );
        drop(held);
        assert!(backing_off(&backoffs, &peer.node_id()));
    }
}
