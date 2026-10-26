//! The hub's streams over QUIC (spec/areas/peer-protocol.md#the-hubs-stream):
//! a stream of kind [`StreamKind::Hub`], [`StreamKind::HubState`] or
//! [`StreamKind::HubFounding`], the
//! request one way, then the stream's end, and the reply back
//! ([`crate::hub::transport`] has the bytes). The client is pinned to the
//! validator's node key, as a peer request is to its node's, and the server
//! learns the client's node from the key the connection proved.

use dassie_peer::wire::Endpoint as PeerEndpoint;
use std::sync::Arc;
use std::time::Duration;

use dassie_identity::NodeId;
use quinn::Connection;

use super::{ANSWER_TIMEOUT, On, Quic, StreamKind, no_room, read_in_order};
use dassie_hub::founding::{FoundingAnswer, Refusal};
use dassie_oer::Encode;

use crate::hub::HubAccess;
use crate::hub::founding::FoundingAccess;
use crate::hub::transport::{
    HUB_ID_BYTES, HubAnswer, HubExchangeError, MAX_FOUNDING_BYTES, MAX_MEMBER_MESSAGE_BYTES,
    MAX_MESSAGE_BYTES, MAX_REPLY_BYTES, MAX_STATE_REPLY_BYTES, STATE_REQUEST_BYTES,
};
use crate::outbound::Resolver;

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
        resolver: &Resolver,
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
        resolver: &Resolver,
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

    /// Sends a sponsor's founding request to `to`, a validator it picked,
    /// on the hub's founding stream, as [`hub_exchange`](Self::hub_exchange)
    /// sends on the hub's stream: the answer's bytes.
    ///
    /// # Errors
    ///
    /// As [`hub_exchange`](Self::hub_exchange).
    pub async fn hub_founding_exchange(
        self: &Arc<Self>,
        to: NodeId,
        endpoint: &str,
        request: &[u8],
        resolver: &Resolver,
        connect_within: Duration,
    ) -> Result<Vec<u8>, HubExchangeError> {
        let route = (to, endpoint);
        let limit = MAX_FOUNDING_BYTES;
        self.hub_stream(
            StreamKind::HubFounding,
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
        (resolver, connect_within): (&Resolver, Duration),
    ) -> Result<Vec<u8>, HubExchangeError> {
        if self.keys.current().is_none() {
            return Err(HubExchangeError::Unreachable);
        }
        let place = self.place(to).ok_or(HubExchangeError::Unreachable)?;
        // A hub's acceptances name an endpoint as `host:port`.
        let endpoint = PeerEndpoint::parse(endpoint).ok_or(HubExchangeError::Unreachable)?;
        // A connection kept from before may have died since: then nothing
        // was sent, and a fresh one is made once.
        for _ in 0..2 {
            let Ok(connection) = self
                .connection(&place, (to, &endpoint), resolver, connect_within, true)
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
    match read_in_order(&mut recv, limit).await {
        Ok(bytes) => Exchanged::Done(Ok(bytes)),
        Err(_) => failed("no reply, or one too long"),
    }
}

impl On {
    /// Answers a request on the hub's stream with the handler, from the
    /// node the connection proved. Nothing is read beyond the hub's ID
    /// until the node validates that hub and a place for the request is
    /// taken; then at most the largest message from a validator of the
    /// hub, and the largest member's request from anyone else.
    pub(super) async fn hub(self, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
        let reset = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        };
        let Some(from) = self.proved else {
            reset(&mut send, &mut recv);
            return;
        };
        let mut hub = [0_u8; HUB_ID_BYTES];
        let read = tokio::time::timeout(REQUEST_TIMEOUT, recv.read_exact(&mut hub)).await;
        let access = match read {
            Ok(Ok(())) => self.handler.hub_access(&from, &hub),
            _ => HubAccess::Unknown,
        };
        let limit = match access {
            HubAccess::Unknown => {
                reset(&mut send, &mut recv);
                return;
            }
            HubAccess::Member => MAX_MEMBER_MESSAGE_BYTES,
            HubAccess::Validator => MAX_MESSAGE_BYTES,
        };
        let Some(_permit) = self.handler.hub_permit(&from, access) else {
            let _ = recv.stop(0u32.into());
            write_reply(&mut send, &HubAnswer::Busy.encode(), ANSWER_TIMEOUT).await;
            return;
        };
        let read = tokio::time::timeout(REQUEST_TIMEOUT, read_in_order(&mut recv, limit)).await;
        let Ok(Ok(message)) = read else {
            reset(&mut send, &mut recv);
            return;
        };
        let mut request = Vec::with_capacity(HUB_ID_BYTES.saturating_add(message.len()));
        request.extend_from_slice(&hub);
        request.extend_from_slice(&message);
        match self.handler.hub(from, request).await {
            Some(answer) => write_reply(&mut send, &answer.encode(), ANSWER_TIMEOUT).await,
            None => reset(&mut send, &mut recv),
        }
    }

    /// Answers a request on the hub's state stream with the handler.
    pub(super) async fn hub_state(self, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
        let reset = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        };
        let Some(from) = self.proved else {
            reset(&mut send, &mut recv);
            return;
        };
        let read = tokio::time::timeout(
            REQUEST_TIMEOUT,
            read_in_order(&mut recv, STATE_REQUEST_BYTES),
        )
        .await;
        let Ok(Ok(request)) = read else {
            reset(&mut send, &mut recv);
            return;
        };
        let Some(reply) = self.handler.hub_state(from, request).await else {
            reset(&mut send, &mut recv);
            return;
        };
        // The reply holds its place until it is written.
        write_reply(&mut send, &reply.bytes, STATE_TIMEOUT).await;
    }
}

impl On {
    /// Answers a sponsor's request on a hub's founding stream with the
    /// handler, from the node the connection proved. Nothing is read until
    /// the handler says the node founds hubs with that node and a place
    /// for the request is taken: a stranger's stream is reset unread, and
    /// a listed sponsor with no place free is answered busy. Then at most
    /// the largest founding request is read.
    pub(super) async fn hub_founding(
        self,
        mut send: quinn::SendStream,
        mut recv: quinn::RecvStream,
    ) {
        let reset = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        };
        let Some(from) = self.proved else {
            reset(&mut send, &mut recv);
            return;
        };
        let place = match self.handler.hub_founding_access(&from) {
            FoundingAccess::NotListed => {
                reset(&mut send, &mut recv);
                return;
            }
            FoundingAccess::Busy => {
                let _ = recv.stop(0u32.into());
                let busy = FoundingAnswer::Refused(Refusal::Busy).to_vec();
                write_reply(&mut send, &busy, ANSWER_TIMEOUT).await;
                return;
            }
            FoundingAccess::Place(place) => place,
        };
        let read = tokio::time::timeout(
            REQUEST_TIMEOUT,
            read_in_order(&mut recv, MAX_FOUNDING_BYTES),
        )
        .await;
        let Ok(Ok(request)) = read else {
            reset(&mut send, &mut recv);
            return;
        };
        match self.handler.hub_founding(from, request, place).await {
            Some(answer) => write_reply(&mut send, &answer, ANSWER_TIMEOUT).await,
            None => reset(&mut send, &mut recv),
        }
    }
}

/// Writes `reply` and ends the stream; a client that does not read it holds
/// nothing for longer than `within`.
async fn write_reply(send: &mut quinn::SendStream, reply: &[u8], within: Duration) {
    match tokio::time::timeout(within, send.write_all(reply)).await {
        Ok(Ok(())) => {
            let _ = send.finish();
        }
        Ok(Err(_)) => {}
        Err(_) => {
            let _ = send.reset(0u32.into());
        }
    }
}
