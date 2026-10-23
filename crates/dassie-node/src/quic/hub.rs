//! The hub's streams over QUIC (spec/areas/peer-protocol.md#the-hubs-stream):
//! a stream of kind [`StreamKind::Hub`] or [`StreamKind::HubState`], the
//! request one way, then the stream's end, and the reply back
//! ([`crate::hub::transport`] has the bytes). The client is pinned to the
//! validator's node key, as a peer request is to its node's, and the server
//! learns the client's node from the key the connection proved.

use std::sync::Arc;
use std::time::Duration;

use dassie_identity::NodeId;
use quinn::Connection;

use super::{ANSWER_TIMEOUT, On, Quic, StreamKind, keys, no_room};
use crate::hub::transport::{
    HubAnswer, HubExchangeError, MAX_REPLY_BYTES, MAX_REQUEST_BYTES, MAX_STATE_REPLY_BYTES,
    STATE_REQUEST_BYTES,
};
use crate::peer_client::PeerClient;

/// Time for a client to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Time to write a state to a client: a state is far larger than a
/// message, up to [`MAX_STATE_REPLY_BYTES`].
const STATE_TIMEOUT: Duration = Duration::from_secs(60);

/// How one exchange on a connection went.
enum Exchanged {
    /// No stream could be opened: nothing was sent.
    NotSent,
    /// The reply's bytes, or why there are none.
    Done(Result<Vec<u8>, HubExchangeError>),
}

impl Quic {
    /// Sends a request on the hub's stream to `to`, a validator reached at
    /// `endpoint` (`host:port`), and reads its reply, making a connection
    /// within `connect_within` if there is none, pinned to `to`'s key. It
    /// shares the connection the node keeps with `to` for peer requests.
    /// The caller bounds the time the whole exchange takes.
    ///
    /// Unlike a peer request, a dial that fails does not mark the node as
    /// out of reach for the peer protocol's while: validators reach each
    /// other over QUIC alone, and must find one that is back at once.
    ///
    /// # Errors
    ///
    /// [`HubExchangeError::Unreachable`] when no connection could be made,
    /// or the node had no room for it, and nothing was sent. Otherwise what
    /// failed.
    pub async fn hub_exchange(
        self: &Arc<Self>,
        to: NodeId,
        endpoint: &str,
        request: &[u8],
        resolver: &PeerClient,
        connect_within: Duration,
    ) -> Result<Vec<Vec<u8>>, HubExchangeError> {
        let route = (to, endpoint);
        let bytes = self
            .hub_stream(
                StreamKind::Hub,
                route,
                request,
                MAX_REPLY_BYTES,
                (resolver, connect_within),
            )
            .await?;
        match HubAnswer::decode(&bytes) {
            Some(HubAnswer::Taken(messages)) => Ok(messages),
            Some(refused) => Err(HubExchangeError::Refused(refused)),
            None => Err(HubExchangeError::BadReply),
        }
    }

    /// Asks `to`, a validator of a hub, for the state after one of its
    /// final blocks on the hub's state stream, as [`hub_exchange`](Self::hub_exchange)
    /// asks on the hub's stream: the reply's bytes, at most
    /// [`MAX_STATE_REPLY_BYTES`].
    ///
    /// # Errors
    ///
    /// As [`hub_exchange`](Self::hub_exchange).
    pub async fn hub_state_exchange(
        self: &Arc<Self>,
        to: NodeId,
        endpoint: &str,
        request: &[u8],
        resolver: &PeerClient,
        connect_within: Duration,
    ) -> Result<Vec<u8>, HubExchangeError> {
        let route = (to, endpoint);
        let limit = MAX_STATE_REPLY_BYTES;
        self.hub_stream(
            StreamKind::HubState,
            route,
            request,
            limit,
            (resolver, connect_within),
        )
        .await
    }

    async fn hub_stream(
        self: &Arc<Self>,
        kind: StreamKind,
        (to, endpoint): (NodeId, &str),
        request: &[u8],
        limit: usize,
        (resolver, connect_within): (&PeerClient, Duration),
    ) -> Result<Vec<u8>, HubExchangeError> {
        if self.keys.current().is_none() {
            return Err(HubExchangeError::Unreachable);
        }
        let place = self.place(to).ok_or(HubExchangeError::Unreachable)?;
        let url = format!("https://{endpoint}");
        // A connection kept from before may have died since: then nothing
        // was sent, and a fresh one is made once.
        for _ in 0..2 {
            let Ok(connection) = self
                .connection(&place, (to, &url), resolver, connect_within)
                .await
            else {
                return Err(HubExchangeError::Unreachable);
            };
            let exchanged = exchange(&connection, kind, request, limit).await;
            if !matches!(exchanged, Exchanged::Done(Ok(_))) && no_room(&connection) {
                return Err(HubExchangeError::Unreachable);
            }
            match exchanged {
                Exchanged::Done(result) => return result,
                Exchanged::NotSent => {
                    let mut held = place.lock().await;
                    if held
                        .as_ref()
                        .is_some_and(|kept| kept.connection.stable_id() == connection.stable_id())
                    {
                        *held = None;
                    }
                }
            }
        }
        Err(HubExchangeError::Unreachable)
    }
}

/// Sends `request` on a new stream of `kind` on `connection` and reads at
/// most `limit` bytes of the reply.
async fn exchange(
    connection: &Connection,
    kind: StreamKind,
    request: &[u8],
    limit: usize,
) -> Exchanged {
    let Ok((mut send, mut recv)) = connection.open_bi().await else {
        return Exchanged::NotSent;
    };
    let failed = |what: &str| Exchanged::Done(Err(HubExchangeError::Failed(what.into())));
    if send.write_all(&[kind.byte()]).await.is_err()
        || send.write_all(request).await.is_err()
        || send.finish().is_err()
    {
        return failed("the request could not be sent");
    }
    match recv.read_to_end(limit).await {
        Ok(bytes) => Exchanged::Done(Ok(bytes)),
        Err(_) => failed("no reply, or one too long"),
    }
}

impl On {
    /// Answers a request on the hub's stream with the handler, from the
    /// node the connection proved.
    pub(super) async fn hub(self, send: quinn::SendStream, recv: quinn::RecvStream) {
        let handler = std::sync::Arc::clone(&self.handler);
        self.serve_hub(
            send,
            recv,
            (MAX_REQUEST_BYTES, ANSWER_TIMEOUT),
            move |from, request| {
                Box::pin(async move {
                    handler
                        .hub(from, request)
                        .await
                        .map(|answer| answer.encode())
                })
            },
        )
        .await;
    }

    /// Answers a request on the hub's state stream with the handler.
    pub(super) async fn hub_state(self, send: quinn::SendStream, recv: quinn::RecvStream) {
        let handler = std::sync::Arc::clone(&self.handler);
        self.serve_hub(
            send,
            recv,
            (STATE_REQUEST_BYTES, STATE_TIMEOUT),
            move |from, request| Box::pin(async move { handler.hub_state(from, request).await }),
        )
        .await;
    }

    /// Reads a request of at most `limit` bytes, answers it with `answer`,
    /// and writes the reply within `write_within`; resets the stream when
    /// there is none.
    async fn serve_hub<F>(
        self,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
        (limit, write_within): (usize, Duration),
        answer: F,
    ) where
        F: FnOnce(
            dassie_identity::PublicKey,
            Vec<u8>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<u8>>> + Send>>,
    {
        let reset = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        };
        let Some(from) = keys::peer_key(&self.connection) else {
            reset(&mut send, &mut recv);
            return;
        };
        let read = tokio::time::timeout(REQUEST_TIMEOUT, recv.read_to_end(limit)).await;
        let Ok(Ok(request)) = read else {
            reset(&mut send, &mut recv);
            return;
        };
        let Some(reply) = answer(from, request).await else {
            reset(&mut send, &mut recv);
            return;
        };
        // A client that does not read its reply holds nothing for longer
        // than this.
        match tokio::time::timeout(write_within, send.write_all(&reply)).await {
            Ok(Ok(())) => {
                let _ = send.finish();
            }
            Ok(Err(_)) => {}
            Err(_) => {
                let _ = send.reset(0u32.into());
            }
        }
    }
}
