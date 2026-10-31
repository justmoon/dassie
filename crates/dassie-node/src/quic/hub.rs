//! The hub's streams over QUIC (spec/areas/peer-protocol.md#the-hubs-stream):
//! a stream of kind [`StreamKind::Hub`], [`StreamKind::HubState`] or
//! [`StreamKind::HubFounding`], the
//! request one way, then the stream's end, and the reply back
//! ([`crate::hub::transport`] has the bytes); and one of kind
//! [`StreamKind::HubBatch`], a batch of validators' messages one way, and a
//! record answering each back as it comes ([`crate::hub::batch`]). The client is pinned to the
//! validator's node key, as a peer request is to its node's, and the server
//! learns the client's node from the key the connection proved.

use std::sync::Arc;
use std::time::Duration;

use dassie_identity::NodeId;
use quinn::Connection;

use super::{ANSWER_TIMEOUT, On, Quic, StreamKind, no_room, read_in_order};
use dassie_hub::founding::{FoundingAnswer, Refusal};
use dassie_oer::Encode;
use dassie_peer::wire::Endpoint as PeerEndpoint;

use crate::hub::HubAccess;
use crate::hub::batch::{
    BATCH_BUSY, BATCH_TAKEN, BatchAccess, MAX_BATCH_BYTES, MAX_BATCH_FRAMES, MAX_BATCH_REPLY_BYTES,
    RECORD_HEAD, ReplyBudget, decode_record_head,
};
use crate::hub::founding::FoundingAccess;
use crate::hub::transport::{
    HUB_ID_BYTES, HubAnswer, HubExchangeError, MAX_FOUNDING_BYTES, MAX_MEMBER_MESSAGE_BYTES,
    MAX_MESSAGE_BYTES, MAX_REPLY_BYTES, MAX_STATE_REPLY_BYTES, REPLY_TIMEOUT, STATE_REQUEST_BYTES,
};
use crate::outbound::Resolver;

/// Time for a client to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Time to write a state to a client: a state is far larger than a
/// message, up to [`MAX_STATE_REPLY_BYTES`].
const STATE_TIMEOUT: Duration = Duration::from_secs(60);

/// Time for a batch's whole reply: its answers, each within
/// [`REPLY_TIMEOUT`], and writing them, as a single message's reply has
/// [`REPLY_TIMEOUT`] and then [`ANSWER_TIMEOUT`] to be written.
const BATCH_REPLY_TIMEOUT: Duration =
    Duration::from_secs(REPLY_TIMEOUT.as_secs() + ANSWER_TIMEOUT.as_secs());

/// How one exchange on a connection went.
enum Exchanged<T = Vec<u8>> {
    /// No stream could be opened: nothing was sent.
    NotSent,
    /// The reply, or why there is none.
    Done(Result<T, HubExchangeError>),
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

    /// Sends a batch of validators' messages, `frames` of them, to `to`, a
    /// validator reached at `endpoint`, on the hub's batch stream, as
    /// [`hub_exchange`](Self::hub_exchange) sends on the hub's stream, and
    /// hands each frame's answer to `records` as it comes, each frame's
    /// once. It returns once the reply ended.
    ///
    /// # Errors
    ///
    /// As [`hub_exchange`](Self::hub_exchange); the answers that came
    /// before a reply failed are handed over all the same.
    #[allow(clippy::too_many_arguments)]
    pub async fn hub_batch_exchange(
        self: &Arc<Self>,
        to: NodeId,
        endpoint: &str,
        request: &[u8],
        frames: usize,
        records: &tokio::sync::mpsc::Sender<(u8, HubAnswer)>,
        resolver: &Resolver,
        connect_within: Duration,
    ) -> Result<(), HubExchangeError> {
        assert!(
            (1..=MAX_BATCH_FRAMES).contains(&frames) && request.len() <= MAX_BATCH_BYTES,
            "a batch has one to {MAX_BATCH_FRAMES} frames and fits its bound"
        );
        let route = (to, endpoint);
        self.on_hub_connection(route, (resolver, connect_within), |connection| {
            exchange_batch(connection, request, frames, records.clone())
        })
        .await
    }

    async fn hub_stream(
        self: &Arc<Self>,
        kind: StreamKind,
        route: (NodeId, &str),
        request: &[u8],
        limit: usize,
        connect: (&Resolver, Duration),
    ) -> Result<Vec<u8>, HubExchangeError> {
        self.on_hub_connection(route, connect, |connection| {
            exchange(connection, kind, request, limit)
        })
        .await
    }

    /// Runs `exchange` on the connection the node keeps with `to`, making
    /// one if there is none, and on a fresh one once if the kept one died.
    async fn on_hub_connection<T, F>(
        self: &Arc<Self>,
        (to, endpoint): (NodeId, &str),
        (resolver, connect_within): (&Resolver, Duration),
        mut exchange: impl FnMut(Connection) -> F,
    ) -> Result<T, HubExchangeError>
    where
        F: Future<Output = Exchanged<T>>,
    {
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
            let exchanged = exchange(connection.clone()).await;
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
    connection: Connection,
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

/// Sends the batch `request` of `frames` frames on a new stream of
/// `connection`, and hands each record of the reply to `records` as it is
/// read: each frame's once, within the reply's bound.
async fn exchange_batch(
    connection: Connection,
    request: &[u8],
    frames: usize,
    records: tokio::sync::mpsc::Sender<(u8, HubAnswer)>,
) -> Exchanged<()> {
    let Ok((mut send, mut recv)) = connection.open_bi().await else {
        return Exchanged::NotSent;
    };
    let failed = |what: &str| Exchanged::Done(Err(HubExchangeError::Failed(what.into())));
    if send
        .write_all(&[StreamKind::HubBatch.byte()])
        .await
        .is_err()
        || send.write_all(request).await.is_err()
        || send.finish().is_err()
    {
        return failed("the batch could not be sent");
    }
    let mut status = [0_u8; 1];
    if recv.read_exact(&mut status).await.is_err() {
        return failed("no reply");
    }
    match status {
        [BATCH_TAKEN] => {}
        [BATCH_BUSY] => return Exchanged::Done(Err(HubExchangeError::Refused(HubAnswer::Busy))),
        _ => return Exchanged::Done(Err(HubExchangeError::BadReply)),
    }
    let mut seen = [false; MAX_BATCH_FRAMES];
    let mut read = 1_usize;
    loop {
        let mut head = [0_u8; RECORD_HEAD];
        match recv.read_exact(&mut head).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(0)) => return Exchanged::Done(Ok(())),
            Err(_) => return failed("the reply broke off"),
        }
        let Some((index, length)) = decode_record_head(&head) else {
            return Exchanged::Done(Err(HubExchangeError::BadReply));
        };
        let slot = usize::from(index);
        read = read.saturating_add(RECORD_HEAD).saturating_add(length);
        if slot >= frames || seen.get(slot).copied().unwrap_or(true) || read > MAX_BATCH_REPLY_BYTES
        {
            return Exchanged::Done(Err(HubExchangeError::BadReply));
        }
        let mut body = vec![0_u8; length];
        if recv.read_exact(&mut body).await.is_err() {
            return failed("the reply broke off");
        }
        let Some(answer) = HubAnswer::decode(&body) else {
            return Exchanged::Done(Err(HubExchangeError::BadReply));
        };
        if let Some(seen) = seen.get_mut(slot) {
            *seen = true;
        }
        // Each frame is answered once, and the channel holds a batch's.
        let _ = records.try_send((index, answer));
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

impl On {
    /// Answers a batch of validators' messages on the hub's batch stream
    /// with the handler, from the node the connection proved. Nothing is
    /// read until the handler says the node validates a hub with that node
    /// and a place for the batch is taken: a stranger's stream is reset
    /// unread, and a validator with no place free is answered busy. Then at
    /// most the largest batch is read, each frame handed to its hub, and
    /// each answer written as it comes, until every frame is answered or
    /// the client stops reading.
    pub(super) async fn hub_batch(self, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
        let reset = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        };
        let Some(from) = self.proved else {
            reset(&mut send, &mut recv);
            return;
        };
        let _place = match self.handler.hub_batch_access(&from) {
            BatchAccess::Unknown => {
                reset(&mut send, &mut recv);
                return;
            }
            BatchAccess::Busy => {
                let _ = recv.stop(0u32.into());
                write_reply(&mut send, &[BATCH_BUSY], ANSWER_TIMEOUT).await;
                return;
            }
            BatchAccess::Place(place) => place,
        };
        let read =
            tokio::time::timeout(REQUEST_TIMEOUT, read_in_order(&mut recv, MAX_BATCH_BYTES)).await;
        let Ok(Ok(request)) = read else {
            reset(&mut send, &mut recv);
            return;
        };
        let Some(replies) = self.handler.hub_batch(from, &request) else {
            reset(&mut send, &mut recv);
            return;
        };
        // One deadline for the whole reply, its answers and their writes, as
        // a single message's reply has: a batch holds its place no longer
        // than one message on the hub's stream does.
        let stopped = send.stopped();
        let replied = tokio::time::timeout(BATCH_REPLY_TIMEOUT, async {
            tokio::select! {
                () = write_records(&mut send, replies) => {}
                // The client stopped reading: nobody waits for the rest.
                _ = stopped => {}
            }
        })
        .await;
        if replied.is_err() {
            let _ = send.reset(0u32.into());
        }
    }
}

/// Writes the batch's status, then each frame's record as its answer comes,
/// within the reply's bound, and ends the stream.
async fn write_records(send: &mut quinn::SendStream, replies: Vec<crate::hub::batch::FrameReply>) {
    use futures_util::StreamExt;
    if send.write_all(&[BATCH_TAKEN]).await.is_err() {
        return;
    }
    let mut budget = ReplyBudget::default();
    let mut replies: futures_util::stream::FuturesUnordered<_> = replies.into_iter().collect();
    while let Some((index, answer)) = replies.next().await {
        let record = budget.record(index, answer);
        if send.write_all(&record).await.is_err() {
            return;
        }
    }
    let _ = send.finish();
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
