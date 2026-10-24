//! The engine: the node's money path in one task.
//!
//! One tokio task owns the ledger, the connector, the STREAM receiver and
//! the settlement schemes ([`Core`]), so none of them needs a lock and their
//! steps happen in one order, as in a simulation
//! (spec/decisions/tigerstyle.md). Everything else talks to it through an
//! [`Engine`] handle: BTP connections send packets, RPC calls send closures
//! that run on the core.
//!
//! The core itself does no IO. The task here drives it: it takes every queued
//! command, lets the core persist what the batch changed in one transaction,
//! and only then sends the batch's packets and replies. So a fulfill goes
//! upstream only once its transfer is on disk. If the write fails, the core
//! panics without handing anything over; the supervisor restarts the task,
//! and it rebuilds from the database (spec/areas/node.md#persistence:
//! in-flight packets do not survive, and their clients reconnect).
//!
//! The engine starts once the node has an identity: its ILP address comes
//! from the node ID.

pub mod core;
mod links;
mod logging;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dassie_connector::{PacketRecord, TraceEvent};
use dassie_ledger::{Account, AccountId, TraceId};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "testing")]
pub use self::core::HubPeer;
pub use self::core::{
    AssetView, BtpEvent, BtpSession, Changes, Core, CoreParts, CreatedOutgoing, Direction,
    EngineError, EngineEvent, Event, HubHold, HubStatus, IncomingPolicy, LedgerEvent, LedgerView,
    Lie, Measured, Move, NetworkView, NewIncoming, NewOutgoing, NewToken, Outbound, Output,
    PaymentEvent, PeerEvent, PeerView, Prober, RemovalBlocker, SCHEMES, SettlementEvent, Severity,
    TokenView, ledger_realm, ledger_slug, offered,
};

use crate::log::target;
use crate::node::Node;

/// Commands queued for the engine.
const QUEUE: usize = 1024;

/// Commands taken in one batch, before persisting.
const BATCH: usize = 256;

/// Most live packet subscribers.
const MAX_PACKET_SUBSCRIBERS: usize = 16;

/// Packet events a subscriber may fall behind by.
const PACKET_EVENTS: usize = 1024;

/// What to tell a caller once its batch is persisted.
type Reply = Box<dyn FnOnce() + Send>;

/// Work for the core, returning what to tell the caller.
type Call = Box<dyn FnOnce(&mut Core) -> Reply + Send>;

enum Command {
    /// A BTP packet: a Prepare in a Message, or a response in a Response.
    Btp {
        connection: u64,
        request_id: u32,
        /// A slice of the WebSocket message it came in, not a copy.
        packet: bytes::Bytes,
        response: bool,
    },
    /// A BTP connection ended.
    BtpClosed { connection: u64 },
    /// A BTP client authenticates; its messages go to `outbound`.
    BtpConnect {
        token: String,
        outbound: mpsc::Sender<Outbound>,
        reply: oneshot::Sender<Result<BtpSession, EngineError>>,
    },
    /// A peer's request; the reply comes once the batch is persisted.
    PeerInbound {
        body: Vec<u8>,
        source: Option<std::net::IpAddr>,
        reply: oneshot::Sender<dassie_peer::Reply>,
    },
    /// The result of one of our peer requests.
    PeerResponse {
        id: dassie_peer::RequestId,
        result: Result<dassie_peer::Response, dassie_peer::Failure>,
    },
    /// Measure the rate to a receiver; the reply comes once the probe is
    /// done.
    Probe {
        ledger: dassie_ledger::LedgerId,
        destination: String,
        shared_secret: [u8; 32],
        by: Prober,
        reply: oneshot::Sender<ProbeResult>,
    },
    /// Run this on the core.
    Call(Call),
}

/// What a probe measured: a rate and the receiver's asset, or nothing.
type ProbeResult = Result<Option<Measured>, EngineError>;

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Btp { connection, .. } => write!(f, "Btp({connection})"),
            Self::BtpClosed { connection } => write!(f, "BtpClosed({connection})"),
            Self::BtpConnect { .. } => f.write_str("BtpConnect"),
            Self::PeerInbound { .. } => f.write_str("PeerInbound"),
            Self::PeerResponse { id, .. } => write!(f, "PeerResponse({id:?})"),
            Self::Probe { .. } => f.write_str("Probe"),
            Self::Call(_) => f.write_str("Call"),
        }
    }
}

/// How often each kind of state has changed, on disk: live views fetch
/// again when the count they follow moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Versions {
    /// The ledgers, balances, tokens and journal.
    pub ledger: u64,
    /// The node table, peers, routes and registrations.
    pub network: u64,
    /// Open Payments resources.
    pub payments: u64,
    /// The price table, which the rate feed publishes.
    pub prices: u64,
}

/// A handle to the running engine. Cheap to clone; stale once its
/// generation ends.
#[derive(Clone, Debug)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    generation: CancellationToken,
    packets: broadcast::Sender<TraceEvent>,
    versions: watch::Receiver<Versions>,
}

impl Engine {
    /// Ends when this engine stops; a new one may start after.
    #[must_use]
    pub fn generation(&self) -> &CancellationToken {
        &self.generation
    }

    /// Runs `work` on the core and returns its result, once the batch it ran
    /// in is persisted.
    async fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Core) -> Result<T, EngineError> + Send + 'static,
    ) -> Result<T, EngineError> {
        let (reply, result) = oneshot::channel();
        let call: Call = Box::new(move |core: &mut Core| {
            let outcome = work(core);
            Box::new(move || {
                let _ = reply.send(outcome);
            })
        });
        self.commands
            .send(Command::Call(call))
            .await
            .map_err(|_| EngineError::Unavailable)?;
        result.await.map_err(|_| EngineError::Unavailable)?
    }

    /// The ledgers of active schemes.
    pub async fn ledgers(&self) -> Result<Vec<LedgerView>, EngineError> {
        self.call(|core| Ok(core.ledger_views())).await
    }

    /// The ledgers of active schemes, and the external assets they settle
    /// through, as their ledgers say, at one moment.
    pub async fn ledgers_and_assets(
        &self,
    ) -> Result<(Vec<LedgerView>, Vec<AssetView>), EngineError> {
        self.call(|core| Ok((core.ledger_views(), core.asset_views())))
            .await
    }

    /// Activates a settlement scheme.
    pub async fn add_scheme(&self, id: String, config: String) -> Result<LedgerView, EngineError> {
        self.call(move |core| core.add_scheme(&id, config)).await
    }

    /// Deactivates a settlement scheme.
    pub async fn remove_scheme(&self, id: String) -> Result<(), EngineError> {
        self.call(move |core| core.remove_scheme(&id)).await
    }

    /// Adds test money through the stub.
    pub async fn stub_deposit(&self, amount: u64) -> Result<LedgerView, EngineError> {
        self.call(move |core| core.stub_deposit(amount)).await
    }

    /// Asks the faucet of a scheme's test ledger for money.
    pub async fn ask_faucet(&self, scheme: String) -> Result<LedgerView, EngineError> {
        self.call(move |core| core.ask_faucet(&scheme)).await
    }

    /// The active schemes' IDs.
    pub async fn active_schemes(&self) -> Result<Vec<String>, EngineError> {
        self.call(|core| Ok(core.active_schemes())).await
    }

    /// Creates a BTP token, with a spending cap if given.
    pub async fn create_token(
        &self,
        label: String,
        ledger: String,
        cap: Option<dassie_connector::SpendingCap>,
    ) -> Result<NewToken, EngineError> {
        self.call(move |core| core.create_token(label, &ledger, cap))
            .await
    }

    /// Sets a BTP token's spending cap, or lifts it.
    pub async fn set_token_cap(
        &self,
        id: u64,
        cap: Option<dassie_connector::SpendingCap>,
    ) -> Result<TokenView, EngineError> {
        self.call(move |core| core.set_token_cap(id, cap)).await
    }

    /// Follows how often the state has changed.
    #[must_use]
    pub fn versions(&self) -> watch::Receiver<Versions> {
        self.versions.clone()
    }

    /// The BTP tokens not revoked.
    pub async fn tokens(&self) -> Result<Vec<TokenView>, EngineError> {
        self.call(|core| Ok(core.token_views())).await
    }

    /// Revokes a BTP token and disconnects its clients.
    pub async fn revoke_token(&self, id: u64) -> Result<(), EngineError> {
        self.call(move |core| core.revoke_token(id)).await
    }

    /// Creates an incoming payment.
    pub async fn create_incoming_payment(
        &self,
        new: NewIncoming,
    ) -> Result<crate::store::IncomingRow, EngineError> {
        self.call(move |core| core.create_incoming_payment(new))
            .await
    }

    /// New credentials for an incoming payment on `ledger`: a STREAM
    /// connection of its own for whoever reads them.
    pub async fn incoming_credentials(
        &self,
        id: crate::store::PaymentId,
        ledger: dassie_ledger::LedgerId,
    ) -> Result<dassie_stream::Credentials, EngineError> {
        self.call(move |core| core.incoming_credentials(id, ledger))
            .await
    }

    /// Makes an outgoing payment, which starts sending at once.
    pub async fn create_outgoing_payment(
        &self,
        new: NewOutgoing,
    ) -> Result<CreatedOutgoing, EngineError> {
        self.call(move |core| core.create_outgoing_payment(new))
            .await
    }

    /// Completes an incoming payment.
    pub async fn complete_incoming_payment(
        &self,
        id: crate::store::PaymentId,
    ) -> Result<crate::store::IncomingRow, EngineError> {
        self.call(move |core| core.complete_incoming_payment(id))
            .await
    }

    /// Tells live views of payments that something they show changed
    /// outside the engine: grants, which the authorization server writes.
    pub async fn payments_changed(&self) {
        let _ = self
            .call(|core| {
                core.payments_changed();
                Ok(())
            })
            .await;
    }

    /// The network: the node table, peers and registrations.
    pub async fn network(&self) -> Result<NetworkView, EngineError> {
        self.call(|core| Ok(core.network())).await
    }

    /// Ends the peering with `node`, as its owner.
    pub async fn terminate_peering(
        &self,
        node: dassie_identity::NodeId,
    ) -> Result<(), EngineError> {
        self.call(move |core| core.terminate_peering(node)).await
    }

    /// Registers with the bootstrap node `node` now.
    pub async fn register_with_bootstrap(
        &self,
        node: dassie_identity::NodeId,
    ) -> Result<(), EngineError> {
        self.call(move |core| core.register_with_bootstrap(node))
            .await
    }

    /// Bans `node`, ending a peering with it.
    pub async fn ban_node(&self, node: dassie_identity::NodeId) -> Result<(), EngineError> {
        self.call(move |core| core.ban_node(node)).await
    }

    /// Lifts a ban.
    pub async fn unban_node(&self, node: dassie_identity::NodeId) -> Result<(), EngineError> {
        self.call(move |core| core.unban_node(&node)).await
    }

    /// The ledger a payment to `destination` goes out on, and whether it
    /// goes to one of the node's own endpoints; none without a route.
    pub async fn route_of(
        &self,
        destination: String,
    ) -> Result<Option<(dassie_ledger::LedgerId, bool)>, EngineError> {
        self.call(move |core| Ok(core.route_of(&destination))).await
    }

    /// The currencies of the active ledgers, whose rates the node needs.
    pub async fn rate_currencies(&self) -> Result<Vec<&'static str>, EngineError> {
        self.call(|core| Ok(core.rate_currencies())).await
    }

    /// Takes exchange rates just fetched.
    pub async fn set_rates(&self, rates: Vec<crate::store::StoredRate>) -> Result<(), EngineError> {
        self.call(move |core| {
            core.set_rates(&rates);
            Ok(())
        })
        .await
    }

    /// Tells live views of the ledgers to fetch again: what they show
    /// beside the engine changed.
    pub async fn touch_ledgers(&self) -> Result<(), EngineError> {
        self.call(|core| {
            core.ledgers_changed();
            Ok(())
        })
        .await
    }

    /// The exchange rates the connector converts at.
    pub async fn rates(&self) -> Result<dassie_connector::Rates, EngineError> {
        self.call(|core| Ok(core.rates().clone())).await
    }

    /// What the connector charges transit and converts at: the price table
    /// as of the engine's last commit (spec/decisions/passive-rebalancing.md).
    pub async fn price_table(&self) -> Result<dassie_connector::PriceTable, EngineError> {
        self.call(|core| Ok(*core.price_table())).await
    }

    /// Measures what the path from `ledger`'s payer to the receiver at
    /// `destination` delivers per unit sent, with packets the receiver
    /// cannot fulfill (spec/decisions/open-payments.md), for `by`, whose
    /// share of the probes it takes, and the asset the receiver says it is
    /// paid in. Nothing when it could not be measured in time.
    pub async fn probe(
        &self,
        ledger: dassie_ledger::LedgerId,
        destination: String,
        shared_secret: [u8; 32],
        by: Prober,
    ) -> Result<Option<Measured>, EngineError> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .send(Command::Probe {
                ledger,
                destination,
                shared_secret,
                by,
                reply,
            })
            .await
            .map_err(|_| EngineError::Unavailable)?;
        // The core gives up on a probe after its timeout; this waits a
        // little longer, for the step that says so.
        let wait = Duration::from_millis(self::core::PROBE_TIMEOUT_MS + 2_000);
        match tokio::time::timeout(wait, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(EngineError::Unavailable),
            Err(_) => Ok(None),
        }
    }

    /// The owner's peer settings.
    pub async fn peer_settings(&self) -> Result<crate::store::PeerSettings, EngineError> {
        self.call(|core| Ok(core.peer_settings())).await
    }

    /// The node's account in its settlement hub and its funding, while a
    /// hub scheme runs.
    pub async fn hub_status(&self) -> Result<Option<HubStatus>, EngineError> {
        self.call(|core| Ok(core.hub_status())).await
    }

    /// The owner deposits into the hub from the XRPL account.
    pub async fn hub_deposit(&self, amount: u64) -> Result<HubStatus, EngineError> {
        self.call(move |core| core.hub_deposit(amount)).await
    }

    /// Pays `drops` from the XRPL testnet account to `destination`: the
    /// payment that creates the escrow of a hub the node founds. Its ID.
    pub async fn pay_from_xrpl(
        &self,
        destination: String,
        drops: u64,
    ) -> Result<String, EngineError> {
        self.call(move |core| {
            core.pay_from_xrpl(&destination, drops)
                .map(|id| id.as_str().to_owned())
        })
        .await
    }

    /// The rules of the hub the node is a member of, while its scheme
    /// runs.
    pub async fn hub_rules(&self) -> Result<Option<dassie_hub::Rules>, EngineError> {
        self.call(|core| Ok(core.hub_rules())).await
    }

    /// What became of the owner's payment `id` out of the XRPL testnet
    /// account.
    pub async fn xrpl_owner_payment(
        &self,
        id: String,
    ) -> Result<dassie_settlement::OwnerPaymentState, EngineError> {
        self.call(move |core| {
            let id = dassie_ledger::Name::new(&id)
                .map_err(|_| EngineError::Invalid("a payment's ID".into()))?;
            Ok(core.xrpl_owner_payment(id))
        })
        .await
    }

    /// The owner withdraws from the hub to the XRPL account.
    pub async fn hub_withdraw(&self, amount: u64) -> Result<HubStatus, EngineError> {
        self.call(move |core| core.hub_withdraw(amount)).await
    }

    /// The owner gives up the late move between the hub and the XRPL
    /// account.
    pub async fn hub_give_up(&self) -> Result<HubStatus, EngineError> {
        self.call(|core| core.hub_give_up()).await
    }

    /// Turns the node's own moves to and from the hub on or off.
    pub async fn set_hub_automatic(
        &self,
        automatic: bool,
    ) -> Result<Option<HubStatus>, EngineError> {
        self.call(move |core| {
            core.set_hub_automatic(automatic)?;
            Ok(core.hub_status())
        })
        .await
    }

    /// Changes the owner's peer settings.
    pub async fn set_peer_settings(
        &self,
        settings: crate::store::PeerSettings,
    ) -> Result<(), EngineError> {
        self.call(move |core| core.set_peer_settings(settings))
            .await
    }

    /// One ledger account.
    pub async fn account(&self, id: AccountId) -> Result<Option<Account>, EngineError> {
        self.call(move |core| Ok(core.account_of(id))).await
    }

    /// Every ledger account.
    pub async fn accounts(&self) -> Result<Vec<(AccountId, Account)>, EngineError> {
        self.call(|core| Ok(core.account_list())).await
    }

    /// Up to `limit` packets after `after` from the trace.
    pub async fn packets(
        &self,
        after: Option<TraceId>,
        limit: usize,
    ) -> Result<Vec<PacketRecord>, EngineError> {
        self.call(move |core| Ok(core.packets(after, limit))).await
    }

    /// The packets from now on, as the trace records them.
    ///
    /// # Errors
    ///
    /// [`EngineError::Exhausted`] when too many are subscribed.
    pub fn subscribe_packets(&self) -> Result<broadcast::Receiver<TraceEvent>, EngineError> {
        if self.packets.receiver_count() >= MAX_PACKET_SUBSCRIBERS {
            return Err(EngineError::Exhausted("too many packet subscribers".into()));
        }
        Ok(self.packets.subscribe())
    }

    /// Opens a BTP connection for the client with `token`.
    pub async fn btp_connect(
        &self,
        token: String,
        outbound: mpsc::Sender<Outbound>,
    ) -> Result<BtpSession, EngineError> {
        let (reply, result) = oneshot::channel();
        let command = Command::BtpConnect {
            token,
            outbound,
            reply,
        };
        self.commands
            .send(command)
            .await
            .map_err(|_| EngineError::Unavailable)?;
        result.await.map_err(|_| EngineError::Unavailable)?
    }

    /// Passes a BTP client's packet in.
    pub async fn btp_packet(
        &self,
        connection: u64,
        request_id: u32,
        packet: bytes::Bytes,
        response: bool,
    ) {
        let command = Command::Btp {
            connection,
            request_id,
            packet,
            response,
        };
        let _ = self.commands.send(command).await;
    }

    /// Says a BTP connection ended.
    pub async fn btp_closed(&self, connection: u64) {
        let _ = self.commands.send(Command::BtpClosed { connection }).await;
    }

    /// Hands in a peer's request, and waits for the reply, which comes once
    /// what the request changed is on disk.
    pub async fn peer_inbound(
        &self,
        body: Vec<u8>,
        source: Option<std::net::IpAddr>,
    ) -> Result<dassie_peer::Reply, EngineError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::PeerInbound {
                body,
                source,
                reply,
            })
            .await
            .map_err(|_| EngineError::Unavailable)?;
        result.await.map_err(|_| EngineError::Unavailable)
    }
}

/// What the node's link state says about it. A periphery node's QUIC
/// endpoint keeps the connections it dials alive, since they are the only
/// way its uplinks reach it (spec/areas/peer-protocol.md#quic).
fn own_info(node: &Node) -> dassie_peer::OwnInfo {
    let info = node.own_info();
    if let Some(quic) = node.quic() {
        quic.set_keep_alive(info.is_periphery());
    }
    info
}

/// The engine as a supervised service: waits for the node's identity, then
/// runs until shutdown. A panic or error ends the generation; the
/// supervisor starts the next.
///
/// The node is up once the engine's first step is on disk, or, on a node
/// without an identity, at once, since nothing starts until the owner sets
/// it up (spec/areas/node.md#readiness).
pub async fn run(node: Arc<Node>, shutdown: CancellationToken) {
    let mut identity = node.identity();
    if identity.borrow().is_none() {
        node.services().up();
    }
    let key = tokio::select! {
        () = shutdown.cancelled() => return,
        key = identity.wait_for(Option::is_some) => match key {
            Ok(key) => key.clone(),
            Err(_) => return,
        },
    };
    let Some(key) = key else { return };
    // Subscribed before our info is read, so no change in between is lost.
    let config = node.subscribe_config();
    let parts = CoreParts {
        store: node.store_handle(),
        key,
        realm: node.config().realm,
        live: node.live_realm(),
        policies: *node.policies(),
        rng: node.shared_rng(),
        peer_rng: node.shared_rng(),
        own_info: own_info(&node),
        bootstrap: node.bootstrap(),
        uplinks: node.pinned_uplinks(),
    };
    let core = match Core::start(parts, node.now_ms()) {
        Ok(core) => core,
        Err(error) => {
            tracing::error!(target: target::ENGINE, %error, "the ledger could not start");
            return;
        }
    };
    let (commands, receiver) = mpsc::channel(QUEUE);
    let generation = CancellationToken::new();
    let packets = broadcast::Sender::new(PACKET_EVENTS);
    let (versions, versions_receiver) = watch::channel(Versions::default());
    let engine = Engine {
        commands: commands.clone(),
        generation: generation.clone(),
        packets: packets.clone(),
        versions: versions_receiver,
    };
    let _published = Published::new(&node, engine, generation.clone());
    tracing::info!(target: target::ENGINE, address = core.own_address(), "the ledger is running");
    let mut driver = Driver {
        node: &node,
        core,
        generation,
        ledgers: links::Links::default(),
        links: BTreeMap::new(),
        packets,
        outputs: Vec::new(),
        replies: Vec::new(),
        commands,
        peer_replies: BTreeMap::new(),
        next_inbound: 0,
        probe_replies: BTreeMap::new(),
        next_probe: 0,
        versions,
    };
    driver.run(receiver, config, &shutdown).await;
}

/// What runs the core: takes commands in batches, steps the core, and
/// carries out what it hands back.
struct Driver<'a> {
    node: &'a Node,
    core: Core,
    /// Ends with this engine: the tasks it starts end with it.
    generation: CancellationToken,
    /// The settlement schemes' connections to their ledgers.
    ledgers: links::Links,
    /// Where each BTP connection's messages go.
    links: BTreeMap<u64, mpsc::Sender<Outbound>>,
    packets: broadcast::Sender<TraceEvent>,
    outputs: Vec<Output>,
    replies: Vec<Reply>,
    /// For the results of peer requests, which come back as commands.
    commands: mpsc::Sender<Command>,
    /// Inbound peer requests waiting for their reply.
    peer_replies: BTreeMap<dassie_peer::InboundId, oneshot::Sender<dassie_peer::Reply>>,
    next_inbound: u64,
    /// Probes waiting for their result.
    probe_replies: BTreeMap<u64, oneshot::Sender<ProbeResult>>,
    next_probe: u64,
    /// What changed, counted, for live views.
    versions: watch::Sender<Versions>,
}

impl Driver<'_> {
    async fn run(
        &mut self,
        mut receiver: mpsc::Receiver<Command>,
        mut config: watch::Receiver<dassie_node_core::config::NodeConfig>,
        shutdown: &CancellationToken,
    ) {
        let mut wake = self.node.clock().subscribe();
        // What starting the core asked for, such as the schemes'
        // connections, goes out now. With this first step on disk, the
        // engine works.
        self.finish();
        self.node.services().up();
        loop {
            let delay = self
                .core
                .deadline()
                .map(|deadline| deadline.saturating_sub(self.node.now_ms()).max(1));
            let sleep = async {
                match delay {
                    Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                command = receiver.recv() => {
                    let Some(command) = command else { break };
                    self.core.begin_step(self.node.now_ms());
                    self.handle(command);
                    for _ in 1..BATCH {
                        match receiver.try_recv() {
                            Ok(command) => self.handle(command),
                            Err(_) => break,
                        }
                    }
                }
                () = sleep => self.core.begin_step(self.node.now_ms()),
                () = clock_moved(&mut wake) => self.core.begin_step(self.node.now_ms()),
                changed = config.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    self.core.begin_step(self.node.now_ms());
                    self.core.set_own_info(own_info(self.node));
                }
            }
            self.finish();
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Btp {
                connection,
                request_id,
                packet,
                response,
            } => self
                .core
                .btp_packet(connection, request_id, &packet, response),
            Command::BtpClosed { connection } => {
                self.links.remove(&connection);
                self.core.btp_closed(connection);
            }
            Command::BtpConnect {
                token,
                outbound,
                reply,
            } => {
                let outcome = self.core.btp_connect(&token);
                if let Ok(session) = &outcome {
                    self.links.insert(session.link, outbound);
                }
                self.replies.push(Box::new(move || {
                    let _ = reply.send(outcome);
                }));
            }
            Command::PeerInbound {
                body,
                source,
                reply,
            } => {
                let inbound = dassie_peer::InboundId(self.next_inbound);
                self.next_inbound += 1;
                self.peer_replies.insert(inbound, reply);
                self.core.peer_inbound(inbound, &body, source);
            }
            Command::PeerResponse { id, result } => self.core.peer_response(id, result),
            Command::Probe {
                ledger,
                destination,
                shared_secret,
                by,
                reply,
            } => {
                let id = self.next_probe;
                self.next_probe += 1;
                match self
                    .core
                    .start_probe(id, ledger, &destination, &shared_secret, by)
                {
                    Ok(()) => {
                        self.probe_replies.insert(id, reply);
                    }
                    Err(error) => self.replies.push(Box::new(move || {
                        let _ = reply.send(Err(error));
                    })),
                }
            }
            Command::Call(call) => {
                let reply = call(&mut self.core);
                self.replies.push(reply);
            }
        }
        self.core.take_journal();
    }

    /// Ends a step: the core persists, then its messages go out, then the
    /// callers hear back. A client that does not read, or whose task is
    /// gone, is disconnected, which is another step.
    fn finish(&mut self) {
        loop {
            self.core.finish_step(&mut self.outputs);
            let failed = self.dispatch();
            if failed.is_empty() {
                break;
            }
            for link in failed {
                self.core.close_link(link);
            }
        }
        let changes = self.core.take_changes();
        if changes != Changes::default() {
            self.versions.send_modify(|versions| {
                versions.ledger += u64::from(changes.ledger);
                versions.network += u64::from(changes.network);
                versions.payments += u64::from(changes.payments);
                versions.prices += u64::from(changes.prices);
            });
        }
        if self.core.take_peerings_changed() {
            let mut classes = crate::quic::places::Classes::default();
            for (node, downlink) in self.core.peerings() {
                let class = if downlink {
                    &mut classes.downlinks
                } else {
                    &mut classes.core
                };
                class.insert(node);
            }
            self.node.set_peerings(classes);
        }
        for reply in self.replies.drain(..) {
            reply();
        }
    }

    /// Hands the step's messages over; returns the links that failed.
    fn dispatch(&mut self) -> Vec<u64> {
        let mut failed = Vec::new();
        for output in std::mem::take(&mut self.outputs) {
            let (link, message) = match output {
                Output::Event(event) => {
                    logging::log(&event);
                    continue;
                }
                Output::Trace(event) => {
                    let _ = self.packets.send(event);
                    continue;
                }
                Output::PeerRequest(request) => {
                    self.send_peer_request(request);
                    continue;
                }
                Output::PeerReply { inbound, reply } => {
                    if let Some(sender) = self.peer_replies.remove(&inbound) {
                        let _ = sender.send(reply);
                    }
                    continue;
                }
                Output::ProbeDone { id, measured } => {
                    if let Some(sender) = self.probe_replies.remove(&id) {
                        let _ = sender.send(Ok(measured));
                    }
                    continue;
                }
                Output::LedgerConnect { .. }
                | Output::LedgerClose { .. }
                | Output::LedgerSend { .. }
                | Output::HubSend { .. }
                | Output::Fund { .. } => {
                    self.ledger_output(output);
                    continue;
                }
                Output::Btp { link, message } => (link, message),
            };
            let closing = message == Outbound::Close;
            let Some(sender) = self.links.get(&link) else {
                continue;
            };
            let failure = match sender.try_send(message) {
                Ok(()) => None,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    Some("a client does not read; disconnecting")
                }
                Err(mpsc::error::TrySendError::Closed(_)) => Some("a client's connection is gone"),
            };
            if closing {
                // Dropping the sender ends the connection's task, if the
                // Close itself could not be queued.
                self.links.remove(&link);
            } else if let Some(failure) = failure
                && !failed.contains(&link)
            {
                tracing::info!(target: target::BTP, link, "{failure}");
                failed.push(link);
            }
        }
        failed
    }
}

impl Driver<'_> {
    /// Carries out what concerns an external asset's ledger.
    fn ledger_output(&mut self, output: Output) {
        match output {
            Output::LedgerConnect { asset, epoch, url } => {
                let client = self.node.peer_client().cloned();
                self.ledgers.connect(
                    client,
                    self.commands.clone(),
                    &self.generation,
                    asset,
                    epoch,
                    url,
                );
            }
            Output::LedgerClose { asset, epoch } => self.ledgers.close(&asset, Some(epoch)),
            Output::LedgerSend {
                asset,
                epoch,
                message,
            } => self.ledgers.send(&asset, epoch, message),
            Output::Fund {
                asset,
                epoch,
                url,
                body,
            } => {
                let client = self.node.peer_client().cloned();
                links::fund(client, self.commands.clone(), asset, epoch, url, body);
            }
            Output::HubSend {
                asset,
                epoch,
                hub,
                endpoint,
                node,
                address,
                message,
            } => links::hub_send(
                crate::hub::HubClient::of(self.node),
                self.node.hubs(),
                self.commands.clone(),
                links::HubSend {
                    asset,
                    epoch,
                    hub,
                    endpoint,
                    node: *node,
                    address,
                    message,
                },
            ),
            _ => {}
        }
    }

    /// Sends a peer request over QUIC on a task of its own; its result
    /// comes back as a command. The peer node bounds the requests in flight.
    fn send_peer_request(&self, request: dassie_peer::Request) {
        let client = self.node.peer_client().cloned();
        let quic = self.node.quic().cloned();
        let commands = self.commands.clone();
        let id = request.id;
        tokio::spawn(async move {
            let (kind, to) = (request.kind, request.to);
            let result = match client {
                Some(client) => crate::quic::send_peer(quic, &client, request).await,
                None => Err(dassie_peer::Failure::NotDelivered),
            };
            let status = result.as_ref().map(|response| response.status);
            tracing::debug!(target: target::PEER, ?kind, %to, ?status, "peer request");
            let _ = commands.send(Command::PeerResponse { id, result }).await;
        });
    }
}

/// Resolves when a clock that can be moved by hand moves, so tests need not
/// wait for real time to pass. Never resolves for the system clock.
async fn clock_moved(wake: &mut Option<watch::Receiver<u64>>) {
    match wake {
        Some(receiver) => {
            if receiver.changed().await.is_err() {
                *wake = None;
            }
        }
        None => std::future::pending().await,
    }
}

/// Publishes the engine on the node while it runs, and withdraws it when the
/// task ends, panics included.
struct Published<'a> {
    node: &'a Node,
    generation: CancellationToken,
}

impl<'a> Published<'a> {
    fn new(node: &'a Node, engine: Engine, generation: CancellationToken) -> Self {
        node.publish_engine(Some(engine));
        Self { node, generation }
    }
}

impl Drop for Published<'_> {
    fn drop(&mut self) {
        self.node.publish_engine(None);
        self.generation.cancel();
    }
}
