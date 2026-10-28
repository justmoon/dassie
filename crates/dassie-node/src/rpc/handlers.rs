//! One function per method.

use std::sync::Arc;

use dassie_node_core::config::DirectoryUrl;
use dassie_node_core::session::SessionId;
use tokio::sync::mpsc;

use super::connection::Queued;
use super::proto::{self, request::Method, response, stream_item};
use super::{Caller, RpcError, convert};
use dassie_identity::NodeId;

use crate::node::{
    MAX_CODE_RELAYS, Node, ObtainError, ObtainRequest, PairingError, Reach, SettingsError,
};
use crate::quic::relay::RelayLink;

/// What a call ended with, short of an error. One per call, moved once:
/// boxing the response would cost an allocation for nothing.
#[allow(clippy::large_enum_variant)]
pub enum Reply {
    /// A unary method's response.
    Unary(response::Method),
    /// A streaming method sent its items and is done.
    StreamEnd,
}

/// Where a streaming method sends its items: dropped when the connection's
/// queue is full ([`send`](Self::send)), for progress reports, or waiting
/// for room ([`send_wait`](Self::send_wait)), for live state, which a lost
/// item would corrupt. A client that stops reading is disconnected.
pub struct Items {
    pub call_id: u32,
    pub generation: u64,
    pub sender: mpsc::Sender<Queued>,
}

impl Items {
    pub(super) fn send(&self, item: stream_item::Method) {
        let _ = self.try_send(item);
    }

    /// Queues an item, waiting for room.
    pub(super) async fn send_wait(&self, item: stream_item::Method) -> Result<(), RpcError> {
        let message = proto::ServerMessage {
            call_id: self.call_id,
            kind: Some(proto::server_message::Kind::Item(proto::StreamItem {
                method: Some(item),
            })),
        };
        self.sender
            .send((self.generation, message))
            .await
            .map_err(|_| RpcError::unavailable("the connection closed"))
    }

    /// Queues an item; false if the connection's queue had no room for it.
    pub(super) fn try_send(&self, item: stream_item::Method) -> bool {
        let message = proto::ServerMessage {
            call_id: self.call_id,
            kind: Some(proto::server_message::Kind::Item(proto::StreamItem {
                method: Some(item),
            })),
        };
        self.sender.try_send((self.generation, message)).is_ok()
    }
}

/// The session behind `caller`, if the call is authenticated: always over
/// IPC, over a WebSocket while its session is valid, and from a device
/// while it is paired (each checked on every call).
fn authenticate(node: &Node, caller: Caller) -> Result<Option<SessionId>, RpcError> {
    match caller {
        Caller::Owner => Ok(None),
        Caller::Browser { session: None, .. } => Err(RpcError::unauthenticated()),
        Caller::Browser {
            session: Some(id), ..
        } => match node.check_session(&id).map_err(RpcError::internal)? {
            Some(id) => Ok(Some(id)),
            None => Err(RpcError::unauthenticated()),
        },
        Caller::Device(device) => {
            if node.check_device(&device).map_err(RpcError::internal)? {
                Ok(None)
            } else {
                Err(RpcError::new(
                    proto::ErrorCode::Unauthenticated,
                    "this device is not paired; claim a pairing first",
                ))
            }
        }
    }
}

/// Whether `caller` may start a pairing, which only the owner on this
/// machine can: over IPC, or in a browser on loopback. A device or a remote
/// browser could otherwise turn its access into lasting access of its own.
const fn may_start_pairing(caller: Caller) -> bool {
    matches!(
        caller,
        Caller::Owner | Caller::Browser { loopback: true, .. }
    )
}

/// Answers one call.
// `GetBasicState` is answered before the caller is authenticated.
#[allow(clippy::unreachable)]
pub async fn dispatch(
    node: &Arc<Node>,
    caller: Caller,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    if let Method::GetBasicState(_) = method {
        return Ok(Reply::Unary(response::Method::GetBasicState(basic_state(
            node, caller,
        ))));
    }
    if let Method::ClaimPairing(request) = method {
        return claim_pairing(node, caller, &request);
    }
    let session = authenticate(node, caller)?;
    let reply = match method {
        Method::GetBasicState(_) | Method::ClaimPairing(_) => unreachable!("answered above"),
        Method::StartPairing(_) => start_pairing(node, caller).await?,
        Method::GetStatus(_) => response::Method::GetStatus(status(node).await),
        Method::GetConfig(_) => response::Method::GetConfig(get_config(node)),
        Method::GetSetupUrl(_) => {
            let url = node
                .setup_url()
                .ok_or_else(|| RpcError::precondition("the node is set up already"))?;
            response::Method::GetSetupUrl(proto::GetSetupUrlResponse { url })
        }
        Method::SetHostname(request) => {
            let config = node
                .set_hostname(&request.hostname)
                .map_err(settings_error)?;
            response::Method::SetHostname(proto::SetHostnameResponse {
                config: Some(convert::config(&config)),
            })
        }
        Method::SetAlias(request) => {
            let config = node.set_alias(&request.alias).map_err(settings_error)?;
            response::Method::SetAlias(proto::SetAliasResponse {
                config: Some(convert::config(&config)),
            })
        }
        other => return dispatch_more(node, session, other, items).await,
    };
    Ok(Reply::Unary(reply))
}

async fn dispatch_more(
    node: &Arc<Node>,
    session: Option<SessionId>,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::SetCertificate(request) => {
            let info = node
                .set_certificate(&request.certificate_chain_pem, &request.private_key_pem)
                .map_err(RpcError::invalid)?;
            response::Method::SetCertificate(proto::SetCertificateResponse {
                certificate: Some(convert::certificate(&info)),
            })
        }
        Method::GetAcmeTerms(request) => {
            let directory = directory(&request.directory_url)?;
            let (directory, terms) = node.acme_terms(directory).await.map_err(obtain_error)?;
            response::Method::GetAcmeTerms(proto::GetAcmeTermsResponse {
                directory_url: directory.as_str().into(),
                terms_of_service_url: terms.unwrap_or_default(),
            })
        }
        Method::ObtainCertificate(request) => {
            return obtain_certificate(node, request, items).await;
        }
        Method::ListSessions(_) => {
            let sessions = node
                .sessions(session.as_ref())
                .map_err(RpcError::internal)?;
            response::Method::ListSessions(proto::ListSessionsResponse {
                sessions: sessions.iter().map(convert::session).collect(),
            })
        }
        Method::RevokeSession(request) => {
            let id = SessionId::parse(&request.id)
                .ok_or_else(|| RpcError::invalid("not a session ID"))?;
            if !node.revoke_session(&id).map_err(RpcError::internal)? {
                return Err(RpcError::not_found("no such session"));
            }
            response::Method::RevokeSession(proto::RevokeSessionResponse {})
        }
        Method::RevokeAllSessions(request) => {
            let keep = if request.keep_current {
                session.as_ref()
            } else {
                None
            };
            let revoked = node.revoke_sessions(keep).map_err(RpcError::internal)?;
            response::Method::RevokeAllSessions(proto::RevokeAllSessionsResponse { revoked })
        }
        Method::ListDevices(_) => {
            let devices = node.devices().map_err(RpcError::internal)?;
            response::Method::ListDevices(proto::ListDevicesResponse {
                devices: devices.iter().map(convert::device).collect(),
            })
        }
        Method::RevokeDevice(request) => {
            let id =
                NodeId::parse(&request.id).map_err(|_| RpcError::invalid("not a device ID"))?;
            if !node.revoke_device(&id).map_err(RpcError::internal)? {
                return Err(RpcError::not_found("no such device"));
            }
            response::Method::RevokeDevice(proto::RevokeDeviceResponse {})
        }
        other => return dispatch_rest(node, other, items).await,
    };
    Ok(Reply::Unary(reply))
}

/// The methods of the services beyond the node's own.
async fn dispatch_rest(node: &Arc<Node>, method: Method, items: &Items) -> Result<Reply, RpcError> {
    if super::founding::is_founding(&method) {
        return super::founding::dispatch(node, method).await;
    }
    match super::inspect::dispatch(node, method, items).await? {
        Ok(reply) => Ok(reply),
        Err(other) => super::ledger::dispatch(node, other, items).await,
    }
}

async fn obtain_certificate(
    node: &Arc<Node>,
    request: proto::ObtainCertificateRequest,
    items: &Items,
) -> Result<Reply, RpcError> {
    let directory = directory(&request.directory_url)?;
    let obtain = ObtainRequest {
        email: request.email,
        agree_to_terms: request.agree_to_terms_of_service,
        directory,
    };
    let mut progress = |step| {
        let step = proto::AcmeProgress {
            step: convert::step(step).into(),
            certificate: None,
        };
        items.send(stream_item::Method::ObtainCertificate(step));
    };
    let info = node
        .obtain_certificate(obtain, &mut progress)
        .await
        .map_err(obtain_error)?;
    items.send(stream_item::Method::ObtainCertificate(
        proto::AcmeProgress {
            step: proto::AcmeStep::Installed.into(),
            certificate: Some(convert::certificate(&info)),
        },
    ));
    Ok(Reply::StreamEnd)
}

/// Starts a pairing, for the owner on the node's own machine.
async fn start_pairing(node: &Node, caller: Caller) -> Result<response::Method, RpcError> {
    if !may_start_pairing(caller) {
        return Err(RpcError::precondition(
            "start a pairing on the node's own machine: in the wallet on localhost, \
             or with `dassie pair`",
        ));
    }
    let reach = reach(node).await;
    let started = node.start_pairing(&reach).map_err(pairing_error)?;
    Ok(response::Method::StartPairing(
        proto::StartPairingResponse {
            code: started.code,
            expires_at_unix_ms: convert::time(started.expires_at_ms),
        },
    ))
}

/// Pairs the device calling, the one call it may make before it is paired.
fn claim_pairing(
    node: &Node,
    caller: Caller,
    request: &proto::ClaimPairingRequest,
) -> Result<Reply, RpcError> {
    let Caller::Device(device) = caller else {
        return Err(RpcError::precondition(
            "only an owner app claims a pairing, on a stream with purpose owner",
        ));
    };
    node.claim_pairing(device, &request.secret, &request.name)
        .map_err(pairing_error)?;
    Ok(Reply::Unary(response::Method::ClaimPairing(
        proto::ClaimPairingResponse {},
    )))
}

/// Where a device reaches the node: at its own URL, where its QUIC
/// endpoint answers; without one, through the peers it is connected to,
/// which relay to it, if its engine runs.
async fn reach(node: &Node) -> Reach {
    let own = node.own_info();
    if let Some(url) = own.url {
        return Reach {
            direct: Some(url),
            relays: Vec::new(),
        };
    }
    let Some(engine) = node.engine() else {
        return Reach::default();
    };
    let Ok(view) = engine.network().await else {
        return Reach::default();
    };
    let relays = view
        .peers
        .iter()
        .filter(|peer| peer.online)
        .filter_map(|peer| {
            let url = view
                .nodes
                .iter()
                .find(|node| node.id == peer.id)?
                .url
                .clone()?;
            Some((peer.id, url))
        })
        .take(MAX_CODE_RELAYS)
        .collect();
    Reach {
        direct: None,
        relays,
    }
}

fn pairing_error(error: PairingError) -> RpcError {
    match error {
        PairingError::NotSetUp => RpcError::precondition(error),
        PairingError::NoPairing => RpcError::not_found(error),
        PairingError::InvalidName => RpcError::invalid(error),
        PairingError::Full => RpcError::exhausted(error),
        PairingError::Store(error) => RpcError::internal(error),
    }
}

fn directory(text: &str) -> Result<Option<DirectoryUrl>, RpcError> {
    if text.is_empty() {
        return Ok(None);
    }
    DirectoryUrl::parse(text)
        .map(Some)
        .map_err(RpcError::invalid)
}

fn basic_state(node: &Node, caller: Caller) -> proto::GetBasicStateResponse {
    let config = node.config();
    let state = match node.node_key() {
        None => proto::NodeState::Uninitialized,
        Some(_) if authenticate(node, caller).is_ok() => proto::NodeState::Authenticated,
        Some(_) => proto::NodeState::Anonymous,
    };
    proto::GetBasicStateResponse {
        state: state.into(),
        node_id: node
            .node_key()
            .map(|key| key.node_id().as_str().to_string())
            .unwrap_or_default(),
        hostname: config
            .hostname
            .map(|name| name.as_str().to_string())
            .unwrap_or_default(),
        ilp_allocation_scheme: config.realm.ilp_allocation_scheme().into(),
        version: crate::VERSION.into(),
        active_settlement_schemes: node
            .store()
            .settlement_schemes()
            .map(|schemes| {
                schemes
                    .iter()
                    .map(|(id, _)| id.as_str().to_string())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// How long status waits for the engine's counters. A stuck engine is when
/// an owner asks for status most, so it answers without them past this.
const COUNTERS_READ_MS: u64 = 1_000;

async fn status(node: &Node) -> proto::GetStatusResponse {
    let (has_account, next_check_ms, last_error) = node.renewal_status();
    let (http_port, https_port) = node.ports();
    let counters = match node.engine() {
        Some(engine) => tokio::time::timeout(
            std::time::Duration::from_millis(COUNTERS_READ_MS),
            engine.counters(),
        )
        .await
        .ok()
        .and_then(Result::ok),
        None => None,
    };
    proto::GetStatusResponse {
        version: crate::VERSION.into(),
        runtime: convert::runtime(node.runtime()).into(),
        started_at_unix_ms: convert::time(node.started_at_ms()),
        certificate: Some(convert::certificate(&node.certificate().info())),
        acme: Some(proto::AcmeStatus {
            has_account,
            next_check_unix_ms: if has_account {
                convert::time(next_check_ms)
            } else {
                0
            },
            last_error: last_error.unwrap_or_default(),
        }),
        http_port: http_port.into(),
        https_port: https_port.into(),
        periphery: node.own_info().is_periphery(),
        relay: node
            .quic()
            .and_then(|quic| quic.chosen_relay())
            .map(|(relay, url, link)| proto::ChosenRelay {
                node: relay.as_str().to_string(),
                url,
                connection: match link {
                    RelayLink::Connected => proto::RelayConnection::Connected,
                    RelayLink::Connecting => proto::RelayConnection::Connecting,
                    RelayLink::Unreachable => proto::RelayConnection::Unreachable,
                }
                .into(),
            }),
        open_payments_origins: crate::open_payments::Names::of(node)
            .map(|names| names.origins())
            .unwrap_or_default(),
        feed: feed_status(node),
        validating: node.hubs().statuses().iter().map(hub_status).collect(),
        founding: super::founding::foundings(node),
        packets: counters.map(|(packets, _)| packet_counters(&packets)),
        peer_protocol: counters.map(|(_, peer)| peer_counters(&peer)),
    }
}

/// The connector's counters (spec/areas/node.md#the-dassie-command).
const fn packet_counters(stats: &dassie_connector::Stats) -> proto::PacketCounters {
    proto::PacketCounters {
        prepares: stats.prepares,
        forwarded: stats.forwarded,
        answered: stats.answered,
        fulfilled: stats.fulfilled,
        rejected: stats.rejected,
        expired: stats.expired,
        wrong_fulfillments: stats.wrong_fulfillments,
        unexpected_responses: stats.unexpected_responses,
        duplicates: stats.duplicates,
    }
}

/// The peer protocol's counters (spec/areas/node.md#the-dassie-command).
const fn peer_counters(stats: &dassie_peer::Stats) -> proto::PeerCounters {
    proto::PeerCounters {
        requests_sent: stats.requests_sent,
        requests_failed: stats.requests_failed,
        requests_dropped: stats.requests_dropped,
        inbound_refused: stats.inbound_refused,
        link_states_accepted: stats.link_states_accepted,
        link_states_dropped: stats.link_states_dropped,
        peerings: stats.peerings,
        terminations: stats.terminations,
        answers_dropped: stats.answers_dropped,
        settlements_delivered: stats.settlements_delivered,
        settlements_applied: stats.settlements_applied,
        settlements_duplicate: stats.settlements_duplicate,
        settlements_requested: stats.settlements_requested,
        channels_closed: stats.channels_closed,
        commitments_logged: stats.commitments_logged,
        commitments_lapsed: stats.commitments_lapsed,
        names_granted: stats.names_granted,
        names_renewed: stats.names_renewed,
        names_freed: stats.names_freed,
        names_released: stats.names_released,
        registrations_started: stats.registrations_started,
        registrations_abandoned: stats.registrations_abandoned,
        renewals_sent: stats.renewals_sent,
    }
}

/// How the node's validator of a hub stands
/// (spec/areas/hubs.md#the-validator-in-the-node).
fn hub_status(status: &crate::hub::HubStatus) -> proto::HubValidator {
    use dassie_hub::ledger::Phase;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    proto::HubValidator {
        hub: crate::hub::driver::hex(status.hub.0.as_bytes()),
        // The set's size when the node is no longer in it.
        index: count(status.index.unwrap_or(status.validators)),
        validators: count(status.validators),
        height: status.height,
        final_unix_ms: convert::time(status.timestamp_ms),
        view: status.view,
        phase: match status.phase {
            Phase::Running => proto::HubPhase::Running,
            Phase::ShuttingDown { .. } => proto::HubPhase::ShuttingDown,
            Phase::Ended => proto::HubPhase::Ended,
        }
        .into(),
        stalled: status.stalled,
        escrow_checked: status.escrow_checked,
        xrpl_connected: status.xrpl_connected,
    }
}

/// The rate feed's state, where the node has its socket: its counters read
/// one by one, without a lock (spec/areas/rate-feed.md#what-the-owner-sees).
fn feed_status(node: &Node) -> Option<proto::RateFeed> {
    let endpoint = node.feed()?;
    let serving = crate::feed::serving(node);
    let stats = endpoint.stats();
    let read =
        |counter: &std::sync::atomic::AtomicU64| counter.load(std::sync::atomic::Ordering::Relaxed);
    // The table holds at most `MAX_SUBSCRIBERS`; those of a feed that does
    // not serve get nothing.
    let subscribers = if serving {
        u32::try_from(read(&stats.subscribers)).unwrap_or(u32::MAX)
    } else {
        0
    };
    Some(proto::RateFeed {
        serving,
        address: endpoint
            .local_addr()
            .map(|address| address.to_string())
            .unwrap_or_default(),
        subscribers,
        published: read(&stats.published),
        sent: read(&stats.sent),
        send_failed: read(&stats.send_failed),
        unsent: read(&stats.unsent),
        received: read(&stats.received),
        challenged: read(&stats.challenged),
        subscribed: read(&stats.subscribed),
        renewed: read(&stats.renewed),
        refused_full: read(&stats.refused_full),
        refused_client: read(&stats.refused_client),
        ignored: read(&stats.ignored),
        expired: read(&stats.expired),
    })
}

fn get_config(node: &Node) -> proto::GetConfigResponse {
    let config = node.config();
    let key = node.node_key();
    proto::GetConfigResponse {
        config: Some(convert::config(&config)),
        has_node_identity: key.is_some(),
        node_id: key
            .map(|key| key.node_id().as_str().to_string())
            .unwrap_or_default(),
        has_tls: node.certificate().info().source.allows_hsts(),
    }
}

fn settings_error(error: SettingsError) -> RpcError {
    match error {
        SettingsError::Invalid(dassie_node_core::config::ConfigError::LiveRealmDisabled) => {
            RpcError::precondition(error)
        }
        SettingsError::Invalid(_) => RpcError::invalid(error),
        SettingsError::Failed(_) => RpcError::internal(error),
    }
}

fn obtain_error(error: ObtainError) -> RpcError {
    match error {
        ObtainError::Precondition(_) | ObtainError::HostnameChanged => {
            RpcError::precondition(error)
        }
        ObtainError::Unavailable(..) => RpcError::unavailable(error),
        ObtainError::Internal(_) => RpcError::internal(error),
    }
}

#[cfg(test)]
mod tests {
    use dassie_identity::IdentityKey;

    use super::*;

    // spec/areas/rpc.md#paired-devices: a stolen session cookie, used from
    // elsewhere, must not become a device's lasting access.
    #[test]
    fn only_the_owner_on_this_machine_may_start_a_pairing() {
        assert!(may_start_pairing(Caller::Owner));
        assert!(may_start_pairing(Caller::Browser {
            session: None,
            loopback: true,
        }));
        assert!(!may_start_pairing(Caller::Browser {
            session: None,
            loopback: false,
        }));
        let device = IdentityKey::from_bytes(&[1; 32]).node_id();
        assert!(!may_start_pairing(Caller::Device(device)));
    }
}
