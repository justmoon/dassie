//! The network's methods: the node table, peers, routes and registration
//! (spec 7 §4.7).

use dassie_peer::{NodeView, Peering, RegistrationStatus, Sources};

use super::handlers::Reply;
use super::proto::{self, request::Method, response};
use super::{RpcError, convert};
use crate::engine::{Engine, NetworkView};

/// Answers the network's methods.
pub async fn dispatch(engine: &Engine, method: Method) -> Result<Reply, RpcError> {
    let view = engine.network().await?;
    let reply = match method {
        Method::ListNodes(_) => response::Method::ListNodes(proto::ListNodesResponse {
            nodes: view.nodes.iter().map(node).collect(),
        }),
        Method::ListPeers(_) => response::Method::ListPeers(proto::ListPeersResponse {
            peers: view
                .peers
                .iter()
                .map(|(id, peering)| peer(id.as_str(), peering))
                .collect(),
        }),
        Method::ListRoutes(_) => response::Method::ListRoutes(proto::ListRoutesResponse {
            routes: routes(&view),
        }),
        Method::GetRegistrationStatus(_) => {
            response::Method::GetRegistrationStatus(proto::GetRegistrationStatusResponse {
                node: view.own.to_string(),
                bootstrap: view.registration.iter().map(bootstrap).collect(),
            })
        }
        _ => unreachable!("the RPC dispatch answers the other methods"),
    };
    Ok(Reply::Unary(reply))
}

fn node(view: &NodeView) -> proto::NetworkNode {
    let sources = view.sources;
    proto::NetworkNode {
        id: view.id.to_string(),
        url: view.url.clone().unwrap_or_default(),
        alias: view.alias.clone().unwrap_or_default(),
        sequence: view.sequence.unwrap_or(0),
        is_self: sources.contains(Sources::SELF),
        bootstrap: sources.contains(Sources::BOOTSTRAP),
        majority: sources.contains(Sources::MAJORITY),
        registered: sources.contains(Sources::REGISTERED),
        peer: sources.contains(Sources::PEER),
        registered_until_unix_ms: view.registered_until_ms.map_or(0, convert::time),
        first_hops: view.first_hops.iter().map(ToString::to_string).collect(),
    }
}

fn peer(id: &str, peering: &Peering) -> proto::NetworkPeer {
    proto::NetworkPeer {
        node: id.to_owned(),
        scheme: peering.scheme.to_string(),
        version: peering.version.into(),
        credit_in: peering.credit_in,
        credit_out: peering.credit_out,
        since_unix_ms: convert::time(peering.since_ms),
        last_seen_unix_ms: convert::time(peering.last_seen_ms),
        inbound: peering.inbound,
    }
}

fn routes(view: &NetworkView) -> Vec<proto::NetworkRoute> {
    view.nodes
        .iter()
        .filter(|node| !node.first_hops.is_empty())
        .map(|node| proto::NetworkRoute {
            node: node.id.to_string(),
            prefix: format!("{}.das.{}", view.allocation_scheme, node.id),
            first_hops: node.first_hops.iter().map(ToString::to_string).collect(),
        })
        .collect()
}

fn bootstrap(status: &RegistrationStatus) -> proto::BootstrapStatus {
    proto::BootstrapStatus {
        node: status.bootstrap.to_string(),
        listed: status.listed,
        registered_at_unix_ms: status.registered_ms.map_or(0, convert::time),
        list_fetched_unix_ms: status.list_fetched_ms.map_or(0, convert::time),
    }
}
