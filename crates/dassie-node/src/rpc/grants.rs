//! Grants for the owner (D9): the one waiting at the consent page, the
//! decision on it, and the grants approved, with what each spent.

use std::sync::Arc;
use std::time::Duration;

use dassie_payments::amount::Amount;
use dassie_payments::gnap::{Access, AccessType, Action};
use dassie_payments::time;
use futures_util::future::join_all;

use super::RpcError;
use super::handlers::{Items, Reply};
use super::live::{self, Follow, Source};
use super::proto::{self, request::Method, response, stream_item};
use crate::engine::Engine;
use crate::node::Node;
use crate::open_payments::auth::OpError;
use crate::open_payments::{Urls, WalletAddresses, consent, spending};
use crate::store::{GrantRow, GrantState, PaymentId};

/// The longest the consent page waits for the receivers a request names,
/// however many steps a read of another node takes. Shorter than one of
/// the client's timeouts, so a node that never answers reads as late.
const RECEIVERS_READ_MS: u64 = 5_000;

/// Most approved grants listed.
const MAX_LISTED: u32 = 256;

/// Answers the grant methods.
pub async fn dispatch(
    node: &Arc<Node>,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let urls = Urls::of(node).ok_or_else(|| RpcError::precondition("the node has no name yet"))?;
    let reply = match method {
        Method::GetGrantRequest(request) => {
            let id = grant_id(&request.id)?;
            let view = consent::describe(node, id, &request.key).map_err(op_error)?;
            let mut grant = grant(node, &urls, &view.grant, &view.access).await?;
            grant.subject = view.subject.unwrap_or_default();
            grant.client_name = view.public_name.unwrap_or_default();
            read_receivers(node, &urls, &mut grant).await;
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
        // so, rather than nothing. An item that does not create spends
        // nothing, and its limits limit only what it reads: its receiver.
        let limits = match item.kind {
            AccessType::OutgoingPayment if !item.actions.contains(&Action::Create) => item
                .limits
                .as_ref()
                .and_then(|limits| limits.receiver.clone())
                .map(|receiver| proto::GrantLimits {
                    receiver,
                    ..proto::GrantLimits::default()
                }),
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
    let span = limits.span();
    Ok(proto::GrantLimits {
        receiver: limits.receiver.clone().unwrap_or_default(),
        interval: limits.interval.clone().unwrap_or_default(),
        debit_amount: limits.debit_amount.as_ref().map(amount),
        receive_amount: limits.receive_amount.as_ref().map(amount),
        window_start_ms: bounded.map(|(start, _)| start),
        window_end_ms: bounded.map(|(_, end)| end),
        spent_debit: debit,
        spent_receive: receive,
        starts_at_ms: span.and_then(|(start, _)| start),
        ends_at_ms: span.and_then(|(_, end)| end),
        receiver_payment: None,
    })
}

/// Reads the receiver each item that creates payments may only pay, so
/// the owner sees what it is paid in and still asks for. Only while the
/// owner is asked: the list of approved grants reads no other node.
///
/// The reads go at once, and all of them together take at most
/// [`RECEIVERS_READ_MS`]: a receiver's node that never answers would
/// otherwise hold the page through each of the client's timeouts.
async fn read_receivers(node: &Node, urls: &Urls, grant: &mut proto::Grant) {
    let wanted: Vec<(usize, String)> = grant
        .access
        .iter()
        .enumerate()
        .filter(|(_, item)| item.actions.iter().any(|action| action == "create"))
        .filter_map(|(index, item)| {
            let receiver = &item.limits.as_ref()?.receiver;
            (!receiver.is_empty()).then(|| (index, receiver.clone()))
        })
        .collect();
    if wanted.is_empty() {
        return;
    }
    let reads = join_all(
        wanted
            .iter()
            .map(|(_, url)| receiver_payment(node, urls, url)),
    );
    let read = tokio::time::timeout(Duration::from_millis(RECEIVERS_READ_MS), reads)
        .await
        .unwrap_or_else(|_| vec![unreadable("its node did not answer in time"); wanted.len()]);
    for ((index, _), payment) in wanted.into_iter().zip(read) {
        if let Some(limits) = grant
            .access
            .get_mut(index)
            .and_then(|item| item.limits.as_mut())
        {
            limits.receiver_payment = Some(payment);
        }
    }
}

/// Why a receiver could not be read, by the status of the failure. The
/// description may quote what the receiver's node sent, so the page shows
/// only what this node says, and the log has the rest.
const fn unreadable_reason(status: u16) -> &'static str {
    match status {
        502 => "its node could not be reached",
        503 => "this node is not ready",
        // This node's own failure, such as its store's.
        500..=599 => "this node could not read it",
        _ => "it is not a payment request that can be read",
    }
}

/// A receiver that could not be read, and why, in the node's own words.
fn unreadable(why: &str) -> proto::GrantReceiver {
    proto::GrantReceiver {
        error: why.into(),
        ..proto::GrantReceiver::default()
    }
}

/// The receiver at `url`, as it reads now. One that cannot be read is
/// shown as such rather than refusing the request: the owner may still
/// deny it, and the client learns why its payments fail when it pays.
async fn receiver_payment(node: &Node, urls: &Urls, url: &str) -> proto::GrantReceiver {
    let payment = match spending::incoming_payment(node, urls, url).await {
        Ok(payment) => payment,
        Err(failure) => {
            tracing::info!(
                target: crate::log::target::PAYMENT,
                receiver = url,
                error = %failure.description,
                "a pending grant's receiver could not be read"
            );
            return unreadable(unreadable_reason(failure.status));
        }
    };
    let received = &payment.received_amount;
    // An expiry that is not a date-time reads as none: paying it refuses
    // it then, as it does any receiver that cannot be paid.
    let expires_at_ms = payment
        .expires_at
        .as_deref()
        .and_then(|expires| time::parse_rfc3339(expires).ok());
    let expired = expires_at_ms.is_some_and(|expires| expires <= node.now_ms());
    proto::GrantReceiver {
        asset_code: received.asset_code.clone(),
        asset_scale: received.asset_scale.into(),
        remaining: payment
            .incoming_amount
            .as_ref()
            .map(|incoming| incoming.value.saturating_sub(received.value)),
        expires_at_ms,
        closed: payment.completed || expired,
        error: String::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Found in review: a failure of this node's own, such as its store's,
    // does not read as the receiver's fault.
    #[test]
    fn unreadable_receivers_say_whose_failure_it_is() {
        assert_eq!(unreadable_reason(502), "its node could not be reached");
        assert_eq!(unreadable_reason(503), "this node is not ready");
        assert_eq!(unreadable_reason(500), "this node could not read it");
        for status in [400, 403, 404] {
            assert_eq!(
                unreadable_reason(status),
                "it is not a payment request that can be read"
            );
        }
    }
}
