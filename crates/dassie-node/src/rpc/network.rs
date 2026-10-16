//! The network's methods: the node table, peers, routes, registration,
//! settlement channels, and ending peerings and banning nodes (spec 7
//! §4.7).

use std::collections::BTreeMap;

use dassie_identity::NodeId;
use dassie_peer::wire::TerminationReason;
use dassie_peer::{Channel, NodeView, PeerStatus, RegistrationStatus, Sources};

use super::handlers::{Items, Reply};
use super::ledger::{permit, source};
use super::live::{self, Follow};
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use crate::engine::{Engine, NetworkView, PeerView};
use crate::node::Node;
use crate::store::{BanRecord, PeerCost, PeerSettings};

/// Answers the network's methods.
pub async fn dispatch(
    node: &Node,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::SubscribeNodes(_) => {
            let _permit = permit(node)?;
            return live::table(
                source(engine),
                items,
                Follow::Network,
                || async { Ok(node_table(&engine.network().await?)) },
                |reset, upserted, removed| {
                    stream_item::Method::SubscribeNodes(proto::NodeTableEvent {
                        reset,
                        upserted,
                        removed,
                    })
                },
            )
            .await;
        }
        Method::SubscribeRoutes(_) => {
            let _permit = permit(node)?;
            return live::values(
                source(engine),
                items,
                Follow::Network,
                || async { Ok(routes(&engine.network().await?)) },
                stream_item::Method::SubscribeRoutes,
            )
            .await;
        }
        Method::SubscribeRegistration(_) => {
            let _permit = permit(node)?;
            return live::values(
                source(engine),
                items,
                Follow::Network,
                || async { Ok(registration(&engine.network().await?)) },
                stream_item::Method::SubscribeRegistration,
            )
            .await;
        }
        Method::RegisterWithBootstrap(request) => {
            engine
                .register_with_bootstrap(node_id(&request.node)?)
                .await?;
            response::Method::RegisterWithBootstrap(proto::RegisterWithBootstrapResponse {})
        }
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
        Method::GetPeerSettings(_) => {
            response::Method::GetPeerSettings(proto::GetPeerSettingsResponse {
                settings: Some(peer_settings(engine.peer_settings().await?)),
            })
        }
        Method::SetPeerSettings(request) => set_peer_settings(engine, request).await?,
        other => view(&engine.network().await?, other),
    };
    Ok(Reply::Unary(reply))
}

/// Changes the owner's peer settings, and answers with them.
async fn set_peer_settings(
    engine: &Engine,
    request: proto::SetPeerSettingsRequest,
) -> Result<response::Method, RpcError> {
    let settings = request
        .settings
        .ok_or_else(|| RpcError::invalid("settings are required"))?;
    engine
        .set_peer_settings(PeerSettings {
            grace_share_ppm: settings.grace_share_ppm,
            connector_fee_ppm: settings.connector_fee_ppm,
            fx_spread_ppm: settings.fx_spread_ppm,
        })
        .await?;
    Ok(response::Method::SetPeerSettings(
        proto::SetPeerSettingsResponse {
            settings: Some(peer_settings(engine.peer_settings().await?)),
        },
    ))
}

/// The methods that only read the network.
// The dispatch sends only the network's methods here.
#[allow(clippy::unreachable)]
fn view(view: &NetworkView, method: Method) -> response::Method {
    match method {
        // A table of 1024 nodes with their neighbors could pass the message
        // limit; SubscribeNodes splits it, a unary answer cannot.
        Method::ListNodes(_) => response::Method::ListNodes(proto::ListNodesResponse {
            nodes: view
                .nodes
                .iter()
                .map(|entry| proto::NetworkNode {
                    neighbors: Vec::new(),
                    settlement_schemes: Vec::new(),
                    ..node(entry)
                })
                .collect(),
        }),
        Method::ListPeers(_) => response::Method::ListPeers(proto::ListPeersResponse {
            peers: view.peers.iter().map(peer).collect(),
        }),
        Method::ListRoutes(_) => response::Method::ListRoutes(routes(view)),
        Method::GetRegistrationStatus(_) => {
            response::Method::GetRegistrationStatus(registration(view))
        }
        Method::ListBans(_) => response::Method::ListBans(proto::ListBansResponse {
            nodes: view.bans.iter().map(|(id, _)| id.to_string()).collect(),
            bans: view
                .bans
                .iter()
                .map(|(id, record)| ban(id, *record))
                .collect(),
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

/// The node table by ID, each node with its peering if there is one.
fn node_table(view: &NetworkView) -> BTreeMap<String, proto::NetworkNode> {
    let peers: BTreeMap<NodeId, &PeerView> = view.peers.iter().map(|p| (p.id, p)).collect();
    let bans: BTreeMap<NodeId, Option<BanRecord>> = view.bans.iter().copied().collect();
    view.nodes
        .iter()
        .map(|entry| {
            let mut message = node(entry);
            message.peering = peers.get(&entry.id).map(|p| peer(p));
            message.ban = bans.get(&entry.id).map(|record| ban(&entry.id, *record));
            (message.id.clone(), message)
        })
        .collect()
}

fn registration(view: &NetworkView) -> proto::GetRegistrationStatusResponse {
    proto::GetRegistrationStatusResponse {
        node: view.own.to_string(),
        bootstrap: view.registration.iter().map(bootstrap).collect(),
        member: view.member,
    }
}

fn node(view: &NodeView) -> proto::NetworkNode {
    let sources = view.sources;
    let peer_state = match view.peer_status {
        PeerStatus::None => proto::PeerState::None,
        PeerStatus::Requesting => proto::PeerState::Requesting,
        PeerStatus::Peered => proto::PeerState::Peered,
        PeerStatus::Terminating => proto::PeerState::Terminating,
    };
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
        neighbors: view.neighbors.iter().map(ToString::to_string).collect(),
        settlement_schemes: view.schemes.iter().map(ToString::to_string).collect(),
        distance: view.distance_hops,
        peer_state: peer_state.into(),
        banned: view.banned,
        peering: None,
        ban: None,
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
        cost: view.cost.as_ref().map(cost),
    }
}

fn cost(cost: &PeerCost) -> proto::PeerCost {
    let asset = cost.ledger.asset();
    proto::PeerCost {
        ledger: cost.ledger.as_str().into(),
        asset_code: asset.code.into(),
        asset_scale: asset.scale.into(),
        refused: cost.refused,
        overdue: cost.overdue,
        grace: cost.grace,
    }
}

/// A ban, with why where that is known: bans from before the node kept
/// reasons have none.
fn ban(node: &NodeId, record: Option<BanRecord>) -> proto::Ban {
    proto::Ban {
        node: node.to_string(),
        reason: record.map_or("", |record| record.reason.as_str()).into(),
        banned_at_unix_ms: record.map_or(0, |record| convert::time(record.banned_at_ms)),
        cost: record.and_then(|record| record.cost).as_ref().map(cost),
    }
}

const fn peer_settings(settings: PeerSettings) -> proto::PeerSettings {
    proto::PeerSettings {
        grace_share_ppm: settings.grace_share_ppm,
        connector_fee_ppm: settings.connector_fee_ppm,
        fx_spread_ppm: settings.fx_spread_ppm,
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

fn routes(view: &NetworkView) -> proto::ListRoutesResponse {
    let routes = view
        .nodes
        .iter()
        .filter(|node| !node.first_hops.is_empty())
        .map(|node| proto::NetworkRoute {
            node: node.id.to_string(),
            prefix: format!("{}.das.{}", view.allocation_scheme, node.id),
            first_hops: node.first_hops.iter().map(ToString::to_string).collect(),
            distance: node.distance_hops.unwrap_or(0),
        })
        .collect();
    let mut local_routes: Vec<proto::LocalRoute> = view
        .local_routes
        .iter()
        .map(|(prefix, endpoint, ledger)| proto::LocalRoute {
            prefix: prefix.clone(),
            endpoint: endpoint.to_string(),
            ledger: ledger.as_str().into(),
        })
        .collect();
    local_routes.sort_by(|a, b| a.prefix.cmp(&b.prefix));
    proto::ListRoutesResponse {
        routes,
        local_routes,
    }
}

fn bootstrap(status: &RegistrationStatus) -> proto::BootstrapStatus {
    proto::BootstrapStatus {
        node: status.bootstrap.to_string(),
        listed: status.listed,
        registered_at_unix_ms: status.registered_ms.map_or(0, convert::time),
        list_fetched_unix_ms: status.list_fetched_ms.map_or(0, convert::time),
    }
}
