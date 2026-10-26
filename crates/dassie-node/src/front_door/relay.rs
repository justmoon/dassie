//! The front door's QUIC endpoint, as the nodes that chose it as their
//! relay see it: it keeps their connections, and answers nothing on them.
//! It takes no peer requests and no relay requests, and serves no relayed
//! stream: it only opens relayed streams to the nodes itself
//! ([`Quic::open_held`](crate::quic::Quic::open_held)).

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;

use dassie_kit::ClientKey;
use dassie_node_core::connections::ConnectionLimits;
use hyper::StatusCode;

use crate::env::MAX_BOOTSTRAP_NODES;
use crate::peer_server::Answer;
use crate::quic::{Body, Handler, MAX_LINKS};

/// Most nodes connected at once: the QUIC endpoint's links but for one
/// per registrar, which the front door dials to read the name lists. So
/// a front door full of nodes still reads them, and every node it takes
/// gets a link, which is how it reaches the node.
pub const MAX_NODES: usize = MAX_LINKS - MAX_BOOTSTRAP_NODES;

/// Most nodes connected at once from one address (an IPv4 address or an
/// IPv6 /56), as the node's listeners allow one client: a few nodes
/// behind one NAT fit.
pub const NODES_PER_ADDRESS: u32 = 64;

/// What the QUIC endpoint answers.
#[derive(Debug)]
pub struct Relay;

impl Handler for Relay {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        Some(ClientKey::from_ip(address.to_canonical()))
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: MAX_NODES,
            max_per_client: NODES_PER_ADDRESS,
        }
    }

    /// A front door is no node: there is nothing for a peer request.
    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        _from: dassie_identity::PublicKey,
        _body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async {
            Answer {
                status: StatusCode::NOT_FOUND.as_u16(),
                body: b"a front door is no node".to_vec(),
            }
        })
    }

    /// It reaches the nodes itself, and carries nobody else's streams.
    fn relays(&self) -> bool {
        false
    }
}
