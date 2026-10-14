//! Grants for the owner (D9): the one waiting at the consent page, the
//! decision on it, and the grants approved, with what each spent.

use std::sync::Arc;

use dassie_payments::amount::Amount;
use dassie_payments::gnap::{Access, AccessType, Action};

use super::RpcError;
use super::handlers::{Items, Reply};
use super::live::{self, Follow, Source};
use super::proto::{self, request::Method, response, stream_item};
use crate::engine::Engine;
use crate::node::Node;
use crate::open_payments::auth::OpError;
use crate::open_payments::{Urls, WalletAddresses, consent};
use crate::store::{GrantRow, GrantState, PaymentId};

/// Most approved grants listed.
const MAX_LISTED: u32 = 256;

/// Answers the grant methods.
pub async fn dispatch(
    node: &Arc<Node>,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let urls = Urls::of(node).ok_or_else(|| RpcError::precondition("the node has no hostname"))?;
    let reply = match method {
        Method::GetGrantRequest(request) => {
            let id = grant_id(&request.id)?;
            let view = consent::describe(node, id, &request.key).map_err(op_error)?;
            let mut grant = grant(node, &urls, &view.grant, &view.access).await?;
            grant.subject = view.subject.unwrap_or_default();
            grant.client_name = view.public_name.unwrap_or_default();
            response::Method::GetGrantRequest(proto::GetGrantRequestResponse { grant: Some(grant) })
        }
        Method::DecideGrant(request) => {
            let id = grant_id(&request.id)?;
            let finish = consent::decide(node, &urls, id, &request.key, request.approve)
                .await
                .map_err(op_error)?;
            response::Method::DecideGrant(proto::DecideGrantResponse {
                finish_url: finish.unwrap_or_default(),
            })
        }
        Method::ListGrants(_) => response::Method::ListGrants(list(node, &urls).await?),
        Method::SubscribeGrants(_) => return subscribe(node, engine, urls, items).await,
        Method::RevokeGrant(request) => {
            let id = grant_id(&request.id)?;
            let row = node
                .store()
                .grant(id)
                .map_err(RpcError::internal)?
                .filter(|row| row.state == GrantState::Approved)
                .ok_or_else(|| RpcError::not_found("no such grant"))?;
            node.store()
                .revoke_grant(row.id, node.now_ms())
                .map_err(RpcError::internal)?;
            engine.payments_changed().await;
            tracing::info!(target: crate::log::target::PAYMENT, grant = %id, "the owner revoked a grant");
            response::Method::RevokeGrant(proto::RevokeGrantResponse {})
        }
        _ => return Err(RpcError::internal("not a grants method")),
    };
    Ok(Reply::Unary(reply))
}

/// Whether a method is one of these.
pub const fn is_grants(method: &Method) -> bool {
    matches!(
        method,
        Method::GetGrantRequest(_)
            | Method::DecideGrant(_)
            | Method::ListGrants(_)
            | Method::SubscribeGrants(_)
            | Method::RevokeGrant(_)
    )
}

fn grant_id(text: &str) -> Result<PaymentId, RpcError> {
    PaymentId::parse(text).ok_or_else(|| RpcError::not_found("no such grant"))
}

fn op_error(error: OpError) -> RpcError {
    error.into()
}

async fn list(node: &Node, urls: &Urls) -> Result<proto::ListGrantsResponse, RpcError> {
    let rows = node
        .store()
        .consented_grants(MAX_LISTED)
        .map_err(RpcError::internal)?;
    let mut grants = Vec::with_capacity(rows.len());
    for row in rows {
        let access: Vec<Access> = serde_json::from_str(&row.access).map_err(RpcError::internal)?;
        grants.push(grant(node, urls, &row, &access).await?);
    }
    Ok(proto::ListGrantsResponse { grants })
}

/// A grant as the owner reads it: its access, and what each
/// outgoing-payment item's limits spent in the current window.
async fn grant(
    node: &Node,
    urls: &Urls,
    row: &GrantRow,
    access: &[Access],
) -> Result<proto::Grant, RpcError> {
    let addresses = WalletAddresses::of(node)
        .await
        .map_err(|_| RpcError::precondition("the ledger is not running"))?;
    let mut items = Vec::with_capacity(access.len());
    for item in access {
        let ledger = item
            .identifier
            .as_deref()
            .and_then(|identifier| addresses.ledger_of_url(urls, identifier));
        // Outgoing payments without limits are of any amount: the page says
        // so, rather than nothing.
        let limits = match item.kind {
            AccessType::OutgoingPayment => {
                let none = dassie_payments::gnap::Limits {
                    receiver: None,
                    interval: None,
                    debit_amount: None,
                    receive_amount: None,
                };
                Some(spent(node, row.id, item.limits.as_ref().unwrap_or(&none))?)
            }
            _ => None,
        };
        items.push(proto::GrantAccess {
            r#type: kind(item.kind).into(),
            actions: item
                .actions
                .iter()
                .map(|action| action_name(*action).into())
                .collect(),
            wallet_address: item.identifier.clone().unwrap_or_default(),
            ledger: ledger
                .map(|ledger| ledger.as_str().to_string())
                .unwrap_or_default(),
            limits,
        });
    }
    Ok(proto::Grant {
        id: row.id.to_string(),
        client: row.client.clone(),
        client_name: String::new(),
        access: items,
        subject: String::new(),
        state: row.state.as_str().into(),
        created_at_ms: row.created_at_ms,
        decided_at_ms: row.decided_at_ms,
        expires_at_ms: row.expires_at_ms,
    })
}

/// A grant's limits, with what it spent in the window that holds now.
fn spent(
    node: &Node,
    grant: PaymentId,
    limits: &dassie_payments::gnap::Limits,
) -> Result<proto::GrantLimits, RpcError> {
    let window = limits.window(node.now_ms());
    let (debit, receive) = match window {
        Some((start, _)) => node
            .store()
            .grant_spend(grant, start)
            .map_err(RpcError::internal)?,
        None => (0, 0),
    };
    // Without an interval, the window is all time: no ends to show.
    let bounded = window.filter(|_| limits.interval.is_some());
    Ok(proto::GrantLimits {
        receiver: limits.receiver.clone().unwrap_or_default(),
        interval: limits.interval.clone().unwrap_or_default(),
        debit_amount: limits.debit_amount.as_ref().map(amount),
        receive_amount: limits.receive_amount.as_ref().map(amount),
        window_start_ms: bounded.map(|(start, _)| start),
        window_end_ms: bounded.map(|(_, end)| end),
        spent_debit: debit,
        spent_receive: receive,
    })
}

fn amount(amount: &Amount) -> proto::OpenPaymentsAmount {
    proto::OpenPaymentsAmount {
        value: amount.value,
        asset_code: amount.asset_code.clone(),
        asset_scale: amount.asset_scale.into(),
    }
}

const fn kind(kind: AccessType) -> &'static str {
    match kind {
        AccessType::IncomingPayment => "incoming-payment",
        AccessType::OutgoingPayment => "outgoing-payment",
        AccessType::Quote => "quote",
    }
}

const fn action_name(action: Action) -> &'static str {
    match action {
        Action::Create => "create",
        Action::Complete => "complete",
        Action::Read => "read",
        Action::ReadAll => "read-all",
        Action::List => "list",
        Action::ListAll => "list-all",
    }
}

/// The approved grants, whole, each time they change.
async fn subscribe(
    node: &Arc<Node>,
    engine: &Engine,
    urls: Urls,
    items: &Items,
) -> Result<Reply, RpcError> {
    let _permit = node
        .live_stream_permit()
        .ok_or_else(|| RpcError::exhausted("too many live streams"))?;
    let source = Source {
        versions: engine.versions(),
        ended: engine.generation().clone(),
    };
    let node = Arc::clone(node);
    let urls = &urls;
    live::values(
        source,
        items,
        Follow::Payments,
        || {
            let node = Arc::clone(&node);
            async move { list(&node, urls).await }
        },
        stream_item::Method::SubscribeGrants,
    )
    .await
}
