//! The engine: the node's money path in one task.
//!
//! One tokio task owns the ledger, the connector, the STREAM receiver and
//! the settlement schemes ([`Core`]), so none of them needs a lock and their
//! steps happen in one order, as in a simulation (D16). Everything else
//! talks to it through an [`Engine`] handle: BTP connections send packets,
//! RPC calls send closures that run on the core.
//!
//! The core itself does no IO. The task here drives it: it takes every
//! queued command, lets the core persist what the batch changed in one
//! transaction, and only then sends the batch's packets and replies. So a
//! fulfill goes upstream only once its transfer is on disk. If the write
//! fails, the core panics without handing anything over; the supervisor
//! restarts the task, and it rebuilds from the database (spec 7 §4.4:
//! in-flight packets do not survive, and their clients reconnect).
//!
//! The engine starts once the node has an identity: its ILP address comes
//! from the node ID.

pub mod core;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dassie_connector::{PacketRecord, TraceEvent};
use dassie_ledger::{Account, AccountId, TraceId};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

pub use self::core::{
    BtpSession, Core, CoreParts, Credentials, EngineError, LedgerView, NetworkView, NewToken,
    Outbound, Output, TokenView,
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
        packet: Vec<u8>,
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
        reply: oneshot::Sender<dassie_peer::Reply>,
    },
    /// The result of one of our peer requests.
    PeerResponse {
        id: dassie_peer::RequestId,
        result: Result<dassie_peer::Response, dassie_peer::Failure>,
    },
    /// Run this on the core.
    Call(Call),
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Btp { connection, .. } => write!(f, "Btp({connection})"),
            Self::BtpClosed { connection } => write!(f, "BtpClosed({connection})"),
            Self::BtpConnect { .. } => f.write_str("BtpConnect"),
            Self::PeerInbound { .. } => f.write_str("PeerInbound"),
            Self::PeerResponse { id, .. } => write!(f, "PeerResponse({id:?})"),
            Self::Call(_) => f.write_str("Call"),
        }
    }
}

/// A handle to the running engine. Cheap to clone; stale once its
/// generation ends.
#[derive(Clone, Debug)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    generation: CancellationToken,
    packets: broadcast::Sender<TraceEvent>,
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

    /// The active schemes' IDs.
    pub async fn active_schemes(&self) -> Result<Vec<String>, EngineError> {
        self.call(|core| Ok(core.active_schemes())).await
    }

    /// Creates a BTP token.
    pub async fn create_token(
        &self,
        label: String,
        ledger: String,
    ) -> Result<NewToken, EngineError> {
        self.call(move |core| core.create_token(label, &ledger))
            .await
    }

    /// The BTP tokens not revoked.
    pub async fn tokens(&self) -> Result<Vec<TokenView>, EngineError> {
        self.call(|core| Ok(core.token_views())).await
    }

    /// Revokes a BTP token and disconnects its clients.
    pub async fn revoke_token(&self, id: u64) -> Result<(), EngineError> {
        self.call(move |core| core.revoke_token(id)).await
    }

    /// Credentials for paying the node's STREAM receiver.
    pub async fn stream_credentials(&self, tag: String) -> Result<Credentials, EngineError> {
        self.call(move |core| core.stream_credentials(&tag)).await
    }

    /// The network: the node table, peers and registrations.
    pub async fn network(&self) -> Result<NetworkView, EngineError> {
        self.call(|core| Ok(core.network())).await
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
        packet: Vec<u8>,
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
    pub async fn peer_inbound(&self, body: Vec<u8>) -> Result<dassie_peer::Reply, EngineError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::PeerInbound { body, reply })
            .await
            .map_err(|_| EngineError::Unavailable)?;
        result.await.map_err(|_| EngineError::Unavailable)
    }
}

/// The engine as a supervised service: waits for the node's identity, then
/// runs until shutdown. A panic or error ends the generation; the
/// supervisor starts the next.
pub async fn run(node: Arc<Node>, shutdown: CancellationToken) {
    let mut identity = node.identity();
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
        policies: *node.policies(),
        rng: node.shared_rng(),
        peer_rng: node.shared_rng(),
        own_info: node.own_info(),
        bootstrap: node.bootstrap(),
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
    let engine = Engine {
        commands: commands.clone(),
        generation: generation.clone(),
        packets: packets.clone(),
    };
    let _published = Published::new(&node, engine, generation);
    tracing::info!(target: target::ENGINE, address = core.own_address(), "the ledger is running");
    let mut driver = Driver {
        node: &node,
        core,
        links: BTreeMap::new(),
        packets,
        outputs: Vec::new(),
        replies: Vec::new(),
        commands,
        peer_replies: BTreeMap::new(),
        next_inbound: 0,
    };
    driver.run(receiver, config, &shutdown).await;
}

/// What runs the core: takes commands in batches, steps the core, and
/// carries out what it hands back.
struct Driver<'a> {
    node: &'a Node,
    core: Core,
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
}

impl Driver<'_> {
    async fn run(
        &mut self,
        mut receiver: mpsc::Receiver<Command>,
        mut config: watch::Receiver<dassie_node_core::config::NodeConfig>,
        shutdown: &CancellationToken,
    ) {
        let mut wake = self.node.clock().subscribe();
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
                    self.core.set_own_info(self.node.own_info());
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
            Command::PeerInbound { body, reply } => {
                let inbound = dassie_peer::InboundId(self.next_inbound);
                self.next_inbound += 1;
                self.peer_replies.insert(inbound, reply);
                self.core.peer_inbound(inbound, &body);
            }
            Command::PeerResponse { id, result } => self.core.peer_response(id, result),
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
        for reply in self.replies.drain(..) {
            reply();
        }
    }

    /// Hands the step's messages over; returns the links that failed.
    fn dispatch(&mut self) -> Vec<u64> {
        let mut failed = Vec::new();
        for output in std::mem::take(&mut self.outputs) {
            let (link, message) = match output {
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
    /// Sends a peer request on a task of its own; its result comes back as
    /// a command. The peer node bounds the requests in flight.
    fn send_peer_request(&self, request: dassie_peer::Request) {
        let client = self.node.peer_client().clone();
        let commands = self.commands.clone();
        let id = request.id;
        tokio::spawn(async move {
            let (kind, to) = (request.kind, request.to);
            let result = client.send(request).await;
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
