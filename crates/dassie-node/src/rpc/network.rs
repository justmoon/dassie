//! The network's methods: the node table, peers, routes, registration,
//! settlement channels, and ending peerings and banning nodes (spec 7
//! §4.7).

use dassie_identity::NodeId;
use dassie_peer::wire::TerminationReason;
use dassie_peer::{Channel, NodeView, RegistrationStatus, Sources};

use super::handlers::Reply;
use super::proto::{self, request::Method, response};
use super::{RpcError, convert};
use crate::engine::{Engine, NetworkView, PeerView};

/// Answers the network's methods.
pub async fn dispatch(engine: &Engine, method: Method) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::TerminatePeering(request) => {
            engine.terminate_peering(node_id(&request.node)?).await?;
            response::Method::TerminatePeering(proto::TerminatePeeringResponse {})
        }
        Method::BanNode(request) => {
            engine.ban_node(node_id(&request.node)?).await?;
            response::Method::BanNode(proto::BanNodeResponse {})
        }
        Method::UnbanNode(request) => {
            engine.unban_node(node_id(&request.node)?).await?;
            response::Method::UnbanNode(proto::UnbanNodeResponse {})
        }
        other => view(&engine.network().await?, other),
    };
    Ok(Reply::Unary(reply))
}

/// The methods that only read the network.
fn view(view: &NetworkView, method: Method) -> response::Method {
    match method {
        Method::ListNodes(_) => response::Method::ListNodes(proto::ListNodesResponse {
            nodes: view.nodes.iter().map(node).collect(),
        }),
        Method::ListPeers(_) => response::Method::ListPeers(proto::ListPeersResponse {
            peers: view.peers.iter().map(peer).collect(),
        }),
        Method::ListRoutes(_) => response::Method::ListRoutes(proto::ListRoutesResponse {
            routes: routes(view),
        }),
        Method::GetRegistrationStatus(_) => {
            response::Method::GetRegistrationStatus(proto::GetRegistrationStatusResponse {
                node: view.own.to_string(),
                bootstrap: view.registration.iter().map(bootstrap).collect(),
            })
        }
        Method::ListBans(_) => response::Method::ListBans(proto::ListBansResponse {
            nodes: view.bans.iter().map(ToString::to_string).collect(),
        }),
        Method::ListSettlementChannels(_) => {
            response::Method::ListSettlementChannels(proto::ListSettlementChannelsResponse {
                channels: view.channels.iter().map(|(id, c)| channel(id, c)).collect(),
            })
        }
        _ => unreachable!("the RPC dispatch answers the other methods"),
    }
}

fn node_id(text: &str) -> Result<NodeId, RpcError> {
    NodeId::parse(text).map_err(|_| RpcError::invalid("not a node ID"))
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

fn peer(view: &PeerView) -> proto::NetworkPeer {
    let peering = &view.peering;
    let (state, reason) = match view.termination {
        Some(termination) => ("terminating", reason(termination.reason)),
        None => ("peered", ""),
    };
    proto::NetworkPeer {
        node: view.id.to_string(),
        scheme: peering.scheme.to_string(),
        version: peering.version.into(),
        credit_in: peering.credit_in,
        credit_out: peering.credit_out,
        since_unix_ms: convert::time(peering.since_ms),
        last_seen_unix_ms: convert::time(peering.last_seen_ms),
        inbound: peering.inbound,
        state: state.into(),
        termination_reason: reason.into(),
        advertised: view.advertised,
        owed: view.owed.map_or(0, |owed| {
            i64::try_from(owed).unwrap_or(if owed < 0 { i64::MIN } else { i64::MAX })
        }),
    }
}

const fn reason(reason: TerminationReason) -> &'static str {
    match reason {
        TerminationReason::Unspecified => "unspecified",
        TerminationReason::Owner => "owner",
        TerminationReason::Unresponsive => "unresponsive",
        TerminationReason::NotPeered => "not-peered",
    }
}

fn channel(id: &NodeId, channel: &Channel) -> proto::SettlementChannel {
    let outbox = channel.outbox.as_ref();
    proto::SettlementChannel {
        node: id.to_string(),
        settled_in: channel.settled_in,
        settled_out: channel.settled_out,
        sending: outbox.is_some(),
        sending_amount: outbox.map_or(0, |o| o.amount),
        sending_attempts: outbox.map_or(0, |o| o.attempts),
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
