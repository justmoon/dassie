//! One RPC connection, over any transport that carries whole messages.
//!
//! Calls run concurrently, up to [`MAX_CALLS_IN_FLIGHT`]. Every call ends
//! with exactly one terminal message (a response, the end of a stream, or
//! an error), even when its handler panics. After a `Cancel`, nothing more
//! is sent for that call. A browser's connection ends when its session
//! does, so logging out takes effect on open sockets too (RT-6); calls
//! already answered get their answers first.
//!
//! The loop never waits on its own outgoing queue, which only it drains:
//! a client that sends faster than it reads, so that even refusals no
//! longer fit, is disconnected. A browser's connection with nothing in
//! flight closes after a while without messages
//! (spec/decisions/tigerstyle.md: an idle socket is memory and a file
//! descriptor held for nothing). A browser's connection without a session,
//! and a device's that was not paired when it connected, also close after a
//! fixed lifetime, whatever they send, so that no one can hold the places for
//! them for ever; like an ending session, the lifetime lets the calls in
//! flight answer first.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{FutureExt, Sink, SinkExt, Stream, StreamExt};
use prost::Message;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;
use tokio::time::{Instant, Sleep};
use tokio_util::sync::CancellationToken;

use super::handlers::{self, Items, Reply};
use super::proto::{self, client_message, server_message};
use super::{Caller, MAX_CALLS_IN_FLIGHT, MAX_STREAMS_IN_FLIGHT, RpcError, is_live};
use crate::log::target;
use crate::node::{Node, Revocation};

/// Messages queued for the client, per connection.
const OUTGOING_QUEUE: usize = 64;

/// How long the client may take to accept one message.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// How long calls already in flight get to answer when the session ends.
const ENDING_GRACE: Duration = Duration::from_secs(1);

/// Why a connection ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// The client went away, or the transport failed.
    ByClient,
    /// The client sent something that is not a `ClientMessage`, or more
    /// than it reads.
    ProtocolError,
    /// The browser's session ended.
    SessionEnded,
    /// Nothing happened for the idle timeout.
    Idle,
    /// A connection without a session reached its lifetime.
    Expired,
    /// The node is shutting down.
    Shutdown,
}

/// A message for the client, from the call of that generation. A call ID
/// can be reused once its call ended, so the generation tells a late message
/// of an old call from one of the new call.
pub type Queued = (u64, proto::ServerMessage);

struct Connection {
    node: Arc<Node>,
    caller: Caller,
    /// Calls in flight: call ID to generation and cancellation.
    calls: BTreeMap<u32, (u64, CancellationToken)>,
    /// Which of them are live streams.
    streams: std::collections::BTreeSet<u32>,
    next_generation: u64,
    tasks: JoinSet<()>,
    sender: mpsc::Sender<Queued>,
    shutdown: CancellationToken,
    /// Set once the session ended or the lifetime is over: when the calls
    /// in flight must be done, and why the connection then closes.
    ending: Option<(Instant, Closed)>,
}

/// Serves the connection until it ends, and says why.
pub async fn serve<In, Out, E>(
    node: Arc<Node>,
    caller: Caller,
    mut incoming: In,
    mut outgoing: Out,
    shutdown: CancellationToken,
) -> Closed
where
    In: Stream<Item = Result<Bytes, E>> + Unpin,
    Out: Sink<Bytes> + Unpin,
{
    let (sender, mut queue) = mpsc::channel(OUTGOING_QUEUE);
    let mut revocations = node.revocations();
    let (idle_after, lifetime) = timeouts(&node, caller);
    let mut connection = Connection::new(node, caller, sender, shutdown);
    let mut idle = pin!(tokio::time::sleep(idle_after.unwrap_or(FAR)));
    // Unlike the idle timer, messages never reset this one.
    let mut expiry = pin!(tokio::time::sleep(lifetime.unwrap_or(FAR)));
    let closed = loop {
        if let Some((deadline, closed)) = connection.ending
            && (connection.calls.is_empty() || Instant::now() >= deadline)
        {
            break closed;
        }
        let watch_idle = idle_after.is_some() && connection.calls.is_empty();
        tokio::select! {
            frame = incoming.next(), if connection.ending.is_none() => match frame {
                Some(Ok(bytes)) => {
                    reset(&mut idle, idle_after);
                    if let Err(closed) = connection.receive(&bytes) {
                        break closed;
                    }
                }
                Some(Err(_)) | None => break Closed::ByClient,
            },
            Some((generation, message)) = queue.recv() => {
                if !connection.forward(generation, &message) {
                    continue;
                }
                reset(&mut idle, idle_after);
                // A client that stops reading would otherwise hold the loop,
                // and with it shutdown, here for ever.
                let send = outgoing.send(Bytes::from(message.encode_to_vec()));
                if !matches!(tokio::time::timeout(SEND_TIMEOUT, send).await, Ok(Ok(()))) {
                    break Closed::ByClient;
                }
            }
            Some(_) = connection.tasks.join_next() => {}
            revocation = revocations.recv() => {
                if connection.ending.is_none() && connection.session_ended(revocation) {
                    connection.ending = Some((Instant::now() + ENDING_GRACE, Closed::SessionEnded));
                }
            }
            () = idle.as_mut(), if watch_idle => break Closed::Idle,
            () = expiry.as_mut(), if lifetime.is_some() && connection.ending.is_none() => {
                connection.ending = Some((Instant::now() + ENDING_GRACE, Closed::Expired));
            }
            () = tokio::time::sleep_until(connection.ending.map_or_else(Instant::now, |(at, _)| at)),
                if connection.ending.is_some() => {}
            () = connection.shutdown.cancelled() => break Closed::Shutdown,
        }
    };
    connection.stop();
    closed
}

/// A timer's duration when it is not used.
const FAR: Duration = Duration::from_secs(86_400 * 365);

/// The idle timeout and the lifetime of `caller`'s connection, if it has
/// them: a browser's or a device's connection goes idle, and one that is not
/// authenticated when it connects (a browser without a session, a device not
/// paired) also has a lifetime, so that it cannot hold its place for ever: a
/// device that pairs connects again. A device's stream also has the relayed
/// streams' own. The owner's IPC connection has neither.
fn timeouts(node: &Node, caller: Caller) -> (Option<Duration>, Option<Duration>) {
    let policies = node.policies();
    let idle = Duration::from_millis(policies.socket_idle_ms);
    let lifetime = Duration::from_millis(policies.anonymous_socket_lifetime_ms);
    match caller {
        Caller::Browser {
            session: Some(_), ..
        } => (Some(idle), None),
        Caller::Device(device) if matches!(node.check_device(&device), Ok(true)) => {
            (Some(idle), None)
        }
        Caller::Browser { session: None, .. } | Caller::Device(_) => (Some(idle), Some(lifetime)),
        Caller::Owner => (None, None),
    }
}

fn reset(idle: &mut std::pin::Pin<&mut Sleep>, after: Option<Duration>) {
    if let Some(after) = after {
        idle.as_mut().reset(Instant::now() + after);
    }
}

impl Connection {
    fn new(
        node: Arc<Node>,
        caller: Caller,
        sender: mpsc::Sender<Queued>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            node,
            caller,
            calls: BTreeMap::new(),
            streams: std::collections::BTreeSet::new(),
            next_generation: 1,
            tasks: JoinSet::new(),
            sender,
            shutdown,
            ending: None,
        }
    }

    /// Cancels the calls still in flight.
    fn stop(&mut self) {
        for (_, call) in self.calls.values() {
            call.cancel();
        }
        self.tasks.abort_all();
    }

    fn receive(&mut self, bytes: &[u8]) -> Result<(), Closed> {
        let Ok(message) = proto::ClientMessage::decode(bytes) else {
            return Err(Closed::ProtocolError);
        };
        let call_id = message.call_id;
        match message.kind {
            Some(client_message::Kind::Request(request)) => self.start(call_id, request),
            Some(client_message::Kind::Cancel(_)) => {
                if let Some((_, call)) = self.calls.remove(&call_id) {
                    call.cancel();
                }
                self.streams.remove(&call_id);
                Ok(())
            }
            None => Err(Closed::ProtocolError),
        }
    }

    fn start(&mut self, call_id: u32, request: proto::Request) -> Result<(), Closed> {
        // Reusing the ID of a call in flight would make every later message
        // ambiguous; the contract forbids it.
        if self.calls.contains_key(&call_id) {
            return Err(Closed::ProtocolError);
        }
        let refusal = if self.calls.len() >= MAX_CALLS_IN_FLIGHT {
            Some(RpcError::new(
                proto::ErrorCode::ResourceExhausted,
                "too many calls in flight",
            ))
        } else if request.method.as_ref().is_some_and(is_live)
            && self.streams.len() >= MAX_STREAMS_IN_FLIGHT
        {
            Some(RpcError::new(
                proto::ErrorCode::ResourceExhausted,
                "too many streams open",
            ))
        } else if request.method.is_none() {
            Some(RpcError::new(
                proto::ErrorCode::Unimplemented,
                "unknown method",
            ))
        } else {
            None
        };
        if let Some(error) = refusal {
            // Generation 0 belongs to no call: the refusal of one that never
            // started. If not even that fits, the client does not read.
            let message = terminal(call_id, Err(error));
            return self
                .sender
                .try_send((0, message))
                .map_err(|_| Closed::ProtocolError);
        }
        let Some(method) = request.method else {
            return Ok(());
        };
        if is_live(&method) {
            self.streams.insert(call_id);
        }
        let token = self.shutdown.child_token();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.calls.insert(call_id, (generation, token.clone()));
        let node = Arc::clone(&self.node);
        let caller = self.caller;
        let items = Items {
            call_id,
            generation,
            sender: self.sender.clone(),
        };
        self.tasks.spawn(async move {
            let work = AssertUnwindSafe(handlers::dispatch(&node, caller, method, &items)).catch_unwind();
            let outcome = tokio::select! {
                () = token.cancelled() => return,
                outcome = work => outcome.unwrap_or_else(|_| Err(RpcError::internal("an RPC handler panicked"))),
            };
            // A task, not the loop, waits here; the loop drains the queue.
            let _ = items.sender.send((generation, terminal(call_id, outcome))).await;
        });
        Ok(())
    }

    /// Whether `message` should go out, retiring its call if it is the
    /// call's last. Messages of cancelled or ended calls are dropped.
    fn forward(&mut self, generation: u64, message: &proto::ServerMessage) -> bool {
        if generation == 0 {
            return true;
        }
        let current = self
            .calls
            .get(&message.call_id)
            .map(|(current, _)| *current);
        if current != Some(generation) {
            return false;
        }
        if !matches!(message.kind, Some(server_message::Kind::Item(_))) {
            self.calls.remove(&message.call_id);
            self.streams.remove(&message.call_id);
        }
        true
    }

    fn session_ended(&self, revocation: Result<Revocation, broadcast::error::RecvError>) -> bool {
        let session = match self.caller {
            Caller::Browser {
                session: Some(session),
                ..
            } => session,
            Caller::Device(device) => return self.device_revoked(device, revocation),
            Caller::Browser { session: None, .. } | Caller::Owner => return false,
        };
        match revocation {
            Ok(Revocation::Recheck) | Err(broadcast::error::RecvError::Lagged(_)) => {
                // Some sessions ended without a name, or notices were
                // missed: ask the database.
                !matches!(self.node.check_session(&session), Ok(Some(_)))
            }
            Ok(revocation) => revocation.ends(&session),
            Err(broadcast::error::RecvError::Closed) => false,
        }
    }

    /// Whether `device` was revoked. A device that has not paired yet ends
    /// only if notices were missed, and connects again.
    fn device_revoked(
        &self,
        device: dassie_identity::NodeId,
        revocation: Result<Revocation, broadcast::error::RecvError>,
    ) -> bool {
        match revocation {
            Ok(Revocation::Device(revoked)) => revoked == device,
            Ok(_) | Err(broadcast::error::RecvError::Closed) => false,
            Err(broadcast::error::RecvError::Lagged(_)) => {
                !matches!(self.node.check_device(&device), Ok(true))
            }
        }
    }
}

fn terminal(call_id: u32, outcome: Result<Reply, RpcError>) -> proto::ServerMessage {
    let kind = match outcome {
        Ok(Reply::Unary(method)) => server_message::Kind::Response(proto::Response {
            method: Some(method),
        }),
        Ok(Reply::StreamEnd) => server_message::Kind::End(proto::StreamEnd {}),
        Err(error) => {
            tracing::debug!(target: target::RPC, call_id, code = ?error.code, message = %error.message, "call failed");
            server_message::Kind::Error(proto::Error {
                code: error.code.into(),
                message: error.message,
            })
        }
    };
    proto::ServerMessage {
        call_id,
        kind: Some(kind),
    }
}
