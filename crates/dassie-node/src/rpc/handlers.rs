//! One function per method.

use std::sync::Arc;

use dassie_node_core::config::{DirectoryUrl, Realm};
use dassie_node_core::session::SessionId;
use tokio::sync::mpsc;

use super::connection::Queued;
use super::proto::{self, request::Method, response, stream_item};
use super::{Caller, RpcError, convert};
use crate::node::{Node, ObtainError, ObtainRequest, SettingsError};

/// What a call ended with, short of an error.
pub enum Reply {
    /// A unary method's response.
    Unary(response::Method),
    /// A streaming method sent its items and is done.
    StreamEnd,
}

/// Where a streaming method sends its items. Items that would overflow the
/// connection's queue are dropped: they are progress reports, and the
/// call's outcome still arrives.
pub struct Items {
    pub call_id: u32,
    pub generation: u64,
    pub sender: mpsc::Sender<Queued>,
}

impl Items {
    pub(super) fn send(&self, item: stream_item::Method) {
        let message = proto::ServerMessage {
            call_id: self.call_id,
            kind: Some(proto::server_message::Kind::Item(proto::StreamItem {
                method: Some(item),
            })),
        };
        let _ = self.sender.try_send((self.generation, message));
    }
}

/// The session behind `caller`, if the call is authenticated: always over
/// IPC, and over a WebSocket while its session is valid (checked on every
/// call, RT-6).
fn authenticate(node: &Node, caller: Caller) -> Result<Option<SessionId>, RpcError> {
    match caller {
        Caller::Owner => Ok(None),
        Caller::Browser(None) => Err(RpcError::unauthenticated()),
        Caller::Browser(Some(id)) => match node.check_session(&id).map_err(RpcError::internal)? {
            Some(id) => Ok(Some(id)),
            None => Err(RpcError::unauthenticated()),
        },
    }
}

/// Answers one call.
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
    let session = authenticate(node, caller)?;
    let reply = match method {
        Method::GetBasicState(_) => unreachable!("answered above"),
        Method::GetStatus(_) => response::Method::GetStatus(status(node)),
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
        Method::SetRealm(request) => {
            let realm = match proto::Realm::try_from(request.realm) {
                Ok(proto::Realm::Test) => Realm::Test,
                Ok(proto::Realm::Live) => Realm::Live,
                _ => return Err(RpcError::invalid("unknown realm")),
            };
            let config = node.set_realm(realm).map_err(settings_error)?;
            response::Method::SetRealm(proto::SetRealmResponse {
                config: Some(convert::config(&config)),
            })
        }
        Method::SetPorts(request) => {
            let (config, restart_required) = node
                .set_ports(request.http_port, request.https_port)
                .map_err(settings_error)?;
            response::Method::SetPorts(proto::SetPortsResponse {
                config: Some(convert::config(&config)),
                restart_required,
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
        other => return super::ledger::dispatch(node, other, items).await,
    };
    Ok(Reply::Unary(reply))
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

fn status(node: &Node) -> proto::GetStatusResponse {
    let (has_account, next_check_ms, last_error) = node.renewal_status();
    let (http_port, https_port) = node.ports();
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
    }
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
