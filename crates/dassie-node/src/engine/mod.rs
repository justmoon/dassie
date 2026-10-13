//! The engine: the node's money path in one task.
//!
//! One tokio task owns the ledger, the connector, the STREAM receiver and
//! the settlement schemes ([`Core`]), so none of them needs a lock and their
//! steps happen in one order, as in a simulation (D16). Everything else
//! talks to it through an [`Engine`] handle: BTP connections send packets,
//! RPC calls send closures that run on the core.
//!
//! The task works in batches: it takes every queued command, then persists
//! what the batch changed in one transaction, and only then sends the
//! batch's packets and replies. So a fulfill goes upstream only once its
//! transfer is on disk. If the write fails, the task panics without sending
//! anything; the supervisor restarts it, and it rebuilds from the database
//! (spec 7 §4.4: in-flight packets do not survive, and their clients
//! reconnect).
//!
//! The engine starts once the node has an identity: its ILP address comes
//! from the node ID.

mod ledgers;
mod packets;
mod state;
mod tokens;

use std::sync::Arc;
use std::time::Duration;

use dassie_connector::{PacketRecord, TraceEvent};
use dassie_ledger::{Account, AccountId, Asset, TraceId};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub use ledgers::LedgerView;
pub use state::Core;
pub use tokens::{BtpSession, NewToken, TokenView};

use crate::log::target;
use crate::node::Node;

/// Commands queued for the engine.
const QUEUE: usize = 1024;

/// Commands taken in one batch, before persisting.
const BATCH: usize = 256;

/// Why the engine did not do something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// The engine is not running: before setup, or restarting.
    Unavailable,
    /// The request is invalid.
    Invalid(String),
    /// The node is not in a state to do it.
    Precondition(String),
    /// No such thing.
    NotFound(String),
    /// A limit is reached.
    Exhausted(String),
    /// A bug; the details are logged.
    Internal(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("the ledger is not running"),
            Self::Invalid(message)
            | Self::Precondition(message)
            | Self::NotFound(message)
            | Self::Exhausted(message)
            | Self::Internal(message) => f.write_str(message),
        }
    }
}

/// What a BTP connection's task sends its client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outbound {
    /// An ILP Prepare, as a BTP Message.
    Prepare {
        /// The request ID.
        request_id: u32,
        /// The packet.
        packet: Vec<u8>,
    },
    /// An ILP Fulfill or Reject, as a BTP Response.
    Response {
        /// The request ID of the Prepare it answers.
        request_id: u32,
        /// The packet.
        packet: Vec<u8>,
    },
    /// A BTP Error for the request.
    Error {
        /// The request ID.
        request_id: u32,
        /// Why.
        message: &'static str,
    },
    /// The connection is over: its token was revoked, or it did not read.
    Close,
}

/// Work for the core.
type Call = Box<dyn FnOnce(&mut Core) + Send>;

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
    /// Run this on the core.
    Call(Call),
}

/// A handle to the running engine. Cheap to clone; stale once its
/// generation ends.
#[derive(Clone, Debug)]
pub struct Engine {
    commands: mpsc::Sender<Command>,
    generation: CancellationToken,
    packets: broadcast::Sender<TraceEvent>,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Btp { connection, .. } => write!(f, "Btp({connection})"),
            Self::BtpClosed { connection } => write!(f, "BtpClosed({connection})"),
            Self::Call(_) => f.write_str("Call"),
        }
    }
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
            core.defer(move || {
                let _ = reply.send(outcome);
            });
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
        self.call(move |core| core.btp_connect(&token, outbound))
            .await
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
}

/// Credentials for the STREAM receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// The address to pay.
    pub destination: String,
    /// The shared secret.
    pub shared_secret: [u8; 32],
    /// What the receiver is paid in.
    pub asset: Asset,
}

/// Most live packet subscribers.
const MAX_PACKET_SUBSCRIBERS: usize = 16;

/// Packet events a subscriber may fall behind by.
const PACKET_EVENTS: usize = 1024;

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
    let mut core = match Core::start(&node, &key) {
        Ok(core) => core,
        Err(error) => {
            tracing::error!(target: target::ENGINE, %error, "the ledger could not start");
            return;
        }
    };
    let (commands, mut receiver) = mpsc::channel(QUEUE);
    let generation = CancellationToken::new();
    let packets = broadcast::Sender::new(PACKET_EVENTS);
    core.set_packet_events(packets.clone());
    let engine = Engine {
        commands,
        generation: generation.clone(),
        packets,
    };
    let _published = Published::new(&node, engine, generation);
    tracing::info!(target: target::ENGINE, address = core.own_address(), "the ledger is running");
    loop {
        let delay = core
            .deadline()
            .map(|deadline| deadline.saturating_sub(node.now_ms()).max(1));
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
                core.handle(command);
                for _ in 1..BATCH {
                    match receiver.try_recv() {
                        Ok(command) => core.handle(command),
                        Err(_) => break,
                    }
                }
            }
            () = sleep => {}
        }
        core.finish_step();
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
