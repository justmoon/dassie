//! The network's methods: the node table, peers, routes, registration,
//! settlement channels, and ending peerings and banning nodes.

use std::collections::BTreeMap;

use dassie_identity::NodeId;
use dassie_peer::wire::TerminationReason;
use dassie_peer::{Channel, NodeView, PeerStatus, RegistrationStatus, Sources};

use super::handlers::{Items, Reply};
use super::ledger::{permit, source};
use super::live::{self, Follow};
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use dassie_connector::Targets;
use dassie_ledger::LedgerId;

use crate::engine::{
    Engine, NetworkView, OwnerSwap, PeerSnapshotView, PeerView, SwapEnd, SwapStats,
};
use crate::node::Node;
use crate::store::PeerSettings;

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
        Method::GetPeerSettings(_)
        | Method::SetPeerSettings(_)
        | Method::GetSwapStatus(_)
        | Method::SwapWithPeer(_)
        | Method::ListPeerSnapshots(_) => peer_settings_and_swaps(engine, method).await?,
        other => view(&engine.network().await?, other),
    };
    Ok(Reply::Unary(reply))
}

/// The owner's peer settings, and the node's swaps with its peers.
// The dispatch sends only these methods here.
#[allow(clippy::unreachable)]
async fn peer_settings_and_swaps(
    engine: &Engine,
    method: Method,
) -> Result<response::Method, RpcError> {
    Ok(match method {
        Method::GetPeerSettings(_) => {
            response::Method::GetPeerSettings(proto::GetPeerSettingsResponse {
                settings: Some(peer_settings(engine.peer_settings().await?)),
            })
        }
        Method::SetPeerSettings(request) => set_peer_settings(engine, request).await?,
        Method::GetSwapStatus(_) => {
            response::Method::GetSwapStatus(swap_status(engine.swap_stats().await?))
        }
        Method::SwapWithPeer(request) => swap_with_peer(engine, request).await?,
        Method::ListPeerSnapshots(_) => {
            response::Method::ListPeerSnapshots(proto::ListPeerSnapshotsResponse {
                snapshots: engine
                    .peer_snapshots()
                    .await?
                    .iter()
                    .map(peer_snapshot)
                    .collect(),
            })
        }
        _ => unreachable!("not a method of the peer settings or swaps"),
    })
}

/// Changes the owner's peer settings, all of them, and answers with them.
async fn set_peer_settings(
    engine: &Engine,
    request: proto::SetPeerSettingsRequest,
) -> Result<response::Method, RpcError> {
    let settings = request
        .settings
        .ok_or_else(|| RpcError::invalid("settings are required"))?;
    let targets: Vec<(LedgerId, u32)> = settings
        .swap_targets
        .iter()
        .map(|target| {
            let ledger = LedgerId::parse(&target.ledger)
                .ok_or_else(|| RpcError::invalid("a swap target names no ledger"))?;
            Ok((ledger, target.target_ppm))
        })
        .collect::<Result<_, RpcError>>()?;
    let swap_targets = Targets::new(&targets).ok_or_else(|| {
        RpcError::invalid("swap targets name each asset once, with a share from 1 to 1 000 000 ppm")
    })?;
    engine
        .set_peer_settings(PeerSettings {
            connector_fee_ppm: settings.connector_fee_ppm,
            fx_spread_ppm: settings.fx_spread_ppm,
            rebalance_ppm: settings.rebalance_ppm,
            offer_swaps: settings.offer_swaps,
            swapping: settings.swapping,
            swap_targets,
        })
        .await?;
    Ok(response::Method::SetPeerSettings(
        proto::SetPeerSettingsResponse {
            settings: Some(peer_settings(engine.peer_settings().await?)),
        },
    ))
}

/// Swaps with a peer for the owner, and answers with what moved.
async fn swap_with_peer(
    engine: &Engine,
    request: proto::SwapWithPeerRequest,
) -> Result<response::Method, RpcError> {
    let peer = node_id(&request.node)?;
    // A rate of 0, as one left out reads, would take whatever the peer's
    // snapshot says.
    if request.rate_numerator == 0 || request.rate_denominator == 0 {
        return Err(RpcError::invalid(
            "a rate is required, and neither of its parts is 0",
        ));
    }
    let ledger = async |scheme: &str| {
        engine
            .method_ledger(scheme.to_owned())
            .await?
            .ok_or_else(|| RpcError::not_found("no such scheme runs"))
    };
    let (pay_on, back_on) = (
        ledger(&request.pay_on).await?,
        ledger(&request.back_on).await?,
    );
    let report = engine
        .swap(OwnerSwap {
            peer,
            pay_on,
            back_on,
            amount: request.amount,
            rate: (request.rate_numerator, request.rate_denominator),
        })
        .await?;
    let ended = match report.ended {
        SwapEnd::Done => proto::SwapEnd::Done,
        SwapEnd::Exhausted => proto::SwapEnd::Exhausted,
        SwapEnd::Refused => proto::SwapEnd::Refused,
        SwapEnd::TimedOut => proto::SwapEnd::TimedOut,
    };
    Ok(response::Method::SwapWithPeer(
        proto::SwapWithPeerResponse {
            paid: report.paid,
            received: report.received,
            fulfilled: report.fulfilled,
            rejected: report.rejected,
            ended: ended.into(),
        },
    ))
}

/// A peer's snapshot, as the contract says it.
fn peer_snapshot(view: &PeerSnapshotView) -> proto::PeerSnapshot {
    let snapshot = &view.snapshot;
    assert_eq!(
        snapshot.methods.len(),
        view.schemes.len(),
        "a scheme per method"
    );
    proto::PeerSnapshot {
        node: view.peer.to_string(),
        sequence: snapshot.sequence_ms,
        valid_until_unix_ms: convert::time(snapshot.valid_until_ms),
        grid_bp: snapshot
            .grid_bp
            .iter()
            .map(|&drop| u32::from(drop))
            .collect(),
        methods: snapshot
            .methods
            .iter()
            .zip(&view.schemes)
            .map(|(method, scheme)| proto::SnapshotMethod {
                tag: method.tag.to_vec(),
                scheme: scheme.clone().unwrap_or_default(),
                bid: method.bid.to_bits(),
                ask: method.ask.to_bits(),
                bid_ladder: method.bid_ladder.amounts().to_vec(),
                ask_ladder: method.ask_ladder.amounts().to_vec(),
            })
            .collect(),
    }
}

/// What the swap service did, as the contract says it.
fn swap_status(stats: SwapStats) -> proto::GetSwapStatusResponse {
    proto::GetSwapStatusResponse {
        by_itself: stats.by_itself,
        peers: stats.peers,
        in_flight: stats.in_flight,
        backing_off: stats.backing_off,
        waiting_for_snapshot: stats.waiting_for_snapshot,
        sent: stats.sent,
        fulfilled: stats.fulfilled,
        rejected: stats.rejected,
        paid_though_rejected: stats.paid_though_rejected,
        backoffs: stats.backoffs,
        refused: stats.refused,
        below_minimum: stats.below_minimum,
        too_large: stats.too_large,
        dropped: stats.dropped,
        moved: stats
            .moved
            .iter()
            .map(|(ledger, &(paid, received))| proto::SwapsMoved {
                ledger: ledger.as_str().to_owned(),
                paid,
                received,
            })
            .collect(),
    }
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

/// A node ID from the owner, which must keep to the wire schema's
/// alphabet: the original's RPC did not check it.
fn node_id(text: &str) -> Result<NodeId, RpcError> {
    NodeId::parse(text).map_err(|_| RpcError::invalid("not a node ID"))
}

/// The node table by ID, each node with its peering if there is one.
fn node_table(view: &NetworkView) -> BTreeMap<String, proto::NetworkNode> {
    let peers: BTreeMap<NodeId, &PeerView> = view.peers.iter().map(|p| (p.id, p)).collect();
    view.nodes
        .iter()
        .map(|entry| {
            let mut message = node(entry);
            message.peering = peers.get(&entry.id).map(|p| peer(p));
            (message.id.clone(), message)
        })
        .collect()
}

fn registration(view: &NetworkView) -> proto::GetRegistrationStatusResponse {
    proto::GetRegistrationStatusResponse {
        node: view.own.to_string(),
        bootstrap: view.registration.iter().map(bootstrap).collect(),
        member: view.member,
        core: view.core,
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
        // A downlink is a peer too, in a place of its own.
        peer: sources.contains(Sources::PEER) || sources.contains(Sources::DOWNLINK),
        registered_until_unix_ms: view.registered_until_ms.map_or(0, convert::time),
        first_hops: view.first_hops.iter().map(ToString::to_string).collect(),
        neighbors: view.neighbors.iter().map(ToString::to_string).collect(),
        settlement_schemes: view.schemes.iter().map(ToString::to_string).collect(),
        distance: view.distance_hops,
        peer_state: peer_state.into(),
        banned: view.banned,
        peering: None,
        core: view.core,
        free_core: view.free_core.map(u32::from),
        refused_until_unix_ms: view.refused_until_ms.map_or(0, convert::time),
    }
}

fn peer(view: &PeerView) -> proto::NetworkPeer {
    let peering = &view.peering;
    // Ending it privately is ending it, to its owner
    // (spec/decisions/ending-a-peering.md).
    let ending = view
        .termination
        .map(|termination| termination.reason)
        .or_else(|| peering.depeering.map(|depeering| depeering.reason));
    let (state, reason) = match ending {
        Some(ending) => ("terminating", reason(ending)),
        None => ("peered", ""),
    };
    proto::NetworkPeer {
        node: view.id.to_string(),
        since_unix_ms: convert::time(peering.since_ms),
        last_seen_unix_ms: convert::time(peering.last_seen_ms),
        inbound: peering.inbound,
        state: state.into(),
        termination_reason: reason.into(),
        advertised: view.advertised,
        online: view.online,
        // Every method, in the order of the peering.
        methods: (peering.methods.iter())
            .zip(&view.owed_by_method)
            .zip(&view.withdrawn_by_method)
            .map(|((method, owed), withdrawn)| proto::PeerMethod {
                scheme: method.scheme.to_string(),
                credit_out: method.credit_out,
                owed: owed.map_or(0, saturate),
                withdrawn: *withdrawn,
                winding_down: method.winding_down(),
            })
            .collect(),
        promotion: view
            .promotion
            .as_ref()
            .map(|(promotion, pending)| proto::Promotion {
                pending: pending
                    .iter()
                    .map(|check| promotion_check(*check).into())
                    .collect(),
                standing_ms: promotion.standing_ms,
                heartbeats_sent: promotion.heartbeats.sent,
                heartbeats_answered: promotion.heartbeats.answered,
                room_packets: promotion.room_packets,
                paid_us: promotion.paid_us,
                clock_skew_ms: promotion.skew_ms,
                url_answered: promotion.url_answered,
                https_answered: promotion.https_answered,
            }),
    }
}

/// A promotion's check as the owner's RPC names it.
const fn promotion_check(check: dassie_peer::PromotionCheck) -> &'static str {
    use dassie_peer::PromotionCheck;
    match check {
        PromotionCheck::Registered => "registered",
        PromotionCheck::Standing => "standing",
        PromotionCheck::Heartbeats => "heartbeats",
        PromotionCheck::Room => "room",
        PromotionCheck::Settled => "settled",
        PromotionCheck::Clock => "clock",
        PromotionCheck::Url => "url",
        PromotionCheck::Https => "https",
        PromotionCheck::NotBanned => "not-banned",
    }
}

fn saturate(owed: i128) -> i64 {
    i64::try_from(owed).unwrap_or(if owed < 0 { i64::MIN } else { i64::MAX })
}

fn peer_settings(settings: PeerSettings) -> proto::PeerSettings {
    proto::PeerSettings {
        connector_fee_ppm: settings.connector_fee_ppm,
        fx_spread_ppm: settings.fx_spread_ppm,
        rebalance_ppm: settings.rebalance_ppm,
        offer_swaps: settings.offer_swaps,
        swapping: settings.swapping,
        swap_targets: settings
            .swap_targets
            .iter()
            .map(|(ledger, target_ppm)| proto::SwapTarget {
                ledger: ledger.as_str().to_owned(),
                target_ppm,
            })
            .collect(),
    }
}

const fn reason(reason: TerminationReason) -> &'static str {
    match reason {
        TerminationReason::Unspecified => "unspecified",
        TerminationReason::Owner => "owner",
        TerminationReason::Unresponsive => "unresponsive",
        TerminationReason::NotPeered => "not-peered",
        TerminationReason::ShortOfRoom => "short-of-room",
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
