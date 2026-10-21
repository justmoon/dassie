//! Owner apps (spec/areas/rpc.md#paired-devices): apps with a device key
//! that call the node's RPC on a relayed stream with purpose `owner`,
//! through a relay or dialled directly. The stream's TLS is pinned to the
//! node's key and proves the device's, so neither a relay nor anyone else
//! reads or forges the calls. The RPC is the wallet's, framed as on the IPC
//! socket; a device is the owner while it is paired.
//!
//! [`serve`] is the node's side; [`connect`] an app's, given an endpoint
//! whose key is the device key.

use std::sync::Arc;

use bytes::BytesMut;
use dassie_identity::NodeId;
use futures_util::StreamExt;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;

use crate::log::target;
use crate::node::Node;
use crate::peer_client::PeerClient;
use crate::quic::Quic;
use crate::quic::relay::{Purpose, RelayError, RelayedStream, ServedStream};
use crate::rpc::{Caller, connection};

/// Serves the RPC to `device` on `stream` until it ends, the device is
/// revoked or `shutdown`.
pub async fn serve(
    node: Arc<Node>,
    stream: ServedStream,
    device: NodeId,
    shutdown: CancellationToken,
) {
    tracing::debug!(target: target::RPC, %device, "owner app connected");
    let (sink, stream) = Framed::new(stream, crate::ipc::codec()).split();
    let incoming = stream.map(|frame| frame.map(BytesMut::freeze));
    let closed = connection::serve(node, Caller::Device(device), incoming, sink, shutdown).await;
    tracing::debug!(target: target::RPC, %device, ?closed, "owner app disconnected");
}

/// Where an app reaches a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route<'a> {
    /// Through the relay `(node ID, URL)`, which the node keeps a
    /// connection to.
    Relay(NodeId, &'a str),
    /// Directly, at the node's QUIC endpoint at this URL.
    Direct(&'a str),
}

/// The RPC's transport to a node, framed as on the IPC socket.
pub type Channel = Framed<RelayedStream, LengthDelimitedCodec>;

/// Opens the RPC to `node` along `route`, from `quic`, whose key is the
/// device key.
///
/// # Errors
///
/// See [`RelayError`]: [`RelayError::Refused`] from a node that serves no
/// owner apps.
pub async fn connect(
    quic: &Arc<Quic>,
    node: NodeId,
    route: Route<'_>,
    resolver: &PeerClient,
) -> Result<Channel, RelayError> {
    let stream = match route {
        Route::Relay(relay, url) => {
            quic.open_relayed((relay, url), node, Purpose::Owner, resolver)
                .await?
        }
        Route::Direct(url) => {
            quic.open_direct((node, url), Purpose::Owner, resolver)
                .await?
        }
    };
    Ok(Framed::new(stream, crate::ipc::codec()))
}
