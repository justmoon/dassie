//! The hub's stream over QUIC (spec/areas/peer-protocol.md#the-hubs-stream):
//! a stream of kind [`StreamKind::Hub`], the request one way, then the
//! stream's end, and the reply back ([`crate::hub::transport`] has the
//! bytes). The client is pinned to the validator's node key, as a peer
//! request is to its node's, and the server learns the client's node from
//! the key the connection proved.

use std::sync::Arc;
use std::time::Duration;

use dassie_identity::NodeId;
use quinn::Connection;

use super::{ANSWER_TIMEOUT, On, Quic, StreamKind, keys, no_room};
use crate::hub::transport::{HubAnswer, HubExchangeError, MAX_REPLY_BYTES, MAX_REQUEST_BYTES};
use crate::peer_client::PeerClient;

/// Time for a client to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How one exchange on a connection went.
enum Exchanged {
    /// No stream could be opened: nothing was sent.
    NotSent,
    /// The reply, or why there is none.
    Done(Result<Vec<Vec<u8>>, HubExchangeError>),
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
            let exchanged = exchange(&connection, request).await;
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

/// Sends `request` on a new hub stream of `connection` and reads the reply.
async fn exchange(connection: &Connection, request: &[u8]) -> Exchanged {
    let Ok((mut send, mut recv)) = connection.open_bi().await else {
        return Exchanged::NotSent;
    };
    let failed = |what: &str| Exchanged::Done(Err(HubExchangeError::Failed(what.into())));
    if send.write_all(&[StreamKind::Hub.byte()]).await.is_err()
        || send.write_all(request).await.is_err()
        || send.finish().is_err()
    {
        return failed("the request could not be sent");
    }
    let Ok(bytes) = recv.read_to_end(1 + MAX_REPLY_BYTES).await else {
        return failed("no reply");
    };
    Exchanged::Done(match HubAnswer::decode(&bytes) {
        Some(HubAnswer::Taken(messages)) => Ok(messages),
        Some(refused) => Err(HubExchangeError::Refused(refused)),
        None => Err(HubExchangeError::BadReply),
    })
}

impl On {
    /// Answers a request on the hub's stream with the handler, from the
    /// node the connection proved.
    pub(super) async fn hub(self, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
        let reset = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        };
        let Some(from) = keys::peer_key(&self.connection) else {
            reset(&mut send, &mut recv);
            return;
        };
        let read = tokio::time::timeout(REQUEST_TIMEOUT, recv.read_to_end(MAX_REQUEST_BYTES)).await;
        let Ok(Ok(request)) = read else {
            reset(&mut send, &mut recv);
            return;
        };
        let Some(answer) = self.handler.hub(from, request).await else {
            reset(&mut send, &mut recv);
            return;
        };
        // A client that does not read its reply holds nothing for longer
        // than this.
        match tokio::time::timeout(ANSWER_TIMEOUT, send.write_all(&answer.encode())).await {
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
