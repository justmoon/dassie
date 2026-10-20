//! The resource server's incoming payments.

use std::net::SocketAddr;

use dassie_ledger::LedgerId;
use dassie_payments::amount::Amount;
use dassie_payments::gnap::{AccessType, Action, Scope};
use dassie_payments::resources::{
    self, CreateIncomingPayment, Direction, IncomingPayment, ListQuery, Page, PageInfo,
    PublicIncomingPayment,
};
use dassie_payments::time;
use dassie_stream::Credentials;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use super::{ReadRequest, error, json};
use crate::engine::{EngineError, NewIncoming};
use crate::http::response::Body;
use crate::node::Node;
use crate::open_payments::auth::{self, Authorized, OpError};
use crate::open_payments::spending::{fresh_credentials, ilp_method};
use crate::open_payments::{Urls, WalletAddresses};
use crate::store::{IncomingQuery, IncomingRow, PaymentId};

/// Routes `/incoming-payments[/{id}[/complete]]`.
pub async fn route(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    request: Request<Incoming>,
) -> Response<Body> {
    let method = request.method().clone();
    let read = match ReadRequest::read(node, urls, request).await {
        Ok(read) => read,
        Err(failure) => return error(&failure, Some(urls)),
    };
    let rest = read
        .path
        .strip_prefix("/incoming-payments")
        .unwrap_or_default();
    let segments: Vec<&str> = rest.split('/').skip(1).collect();
    let result = match (&method, segments.as_slice()) {
        (&Method::POST, []) => create(node, peer, urls, &read).await,
        (&Method::GET, []) => list(node, urls, &read).await,
        (&Method::GET | &Method::HEAD, [id]) => get(node, urls, &read, id).await,
        (&Method::POST, [id, "complete"]) => complete(node, urls, &read, id).await,
        _ => Err(OpError::not_found("Not Found")),
    };
    match result {
        Ok(response) => response,
        Err(failure) => error(&failure, Some(urls)),
    }
}

fn authorize(node: &Node, read: &ReadRequest) -> Result<Authorized, OpError> {
    let fields = read.field_refs();
    auth::authorize(node, &read.urls, &read.signed(&fields))
}

async fn addresses(node: &Node) -> Result<WalletAddresses, OpError> {
    WalletAddresses::of(node)
        .await
        .map_err(|_| OpError::unavailable())
}

pub(super) fn engine_error(error: EngineError) -> OpError {
    match error {
        EngineError::Unavailable => OpError::unavailable(),
        EngineError::Invalid(message) | EngineError::Precondition(message) => {
            OpError::invalid(message)
        }
        EngineError::NotFound(message) => OpError::not_found(message),
        EngineError::Exhausted(message) => OpError::new(429, "too_many_requests", message),
        EngineError::Refused(message) => OpError::new(403, "invalid_request", message),
        EngineError::Internal(message) => OpError::internal(message),
    }
}

/// `POST /incoming-payments`.
async fn create(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    read: &ReadRequest,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let body: CreateIncomingPayment = serde_json::from_slice(&read.body)
        .map_err(|_| OpError::invalid("the incoming payment is malformed"))?;
    let addresses = addresses(node).await?;
    let ledger = addresses
        .ledger_of_url(urls, &body.wallet_address)
        .ok_or_else(|| OpError::invalid("the wallet address is not one of this node"))?;
    authorized
        .permits(urls, AccessType::IncomingPayment, Action::Create, ledger)
        .ok_or_else(|| {
            OpError::forbidden("the grant does not allow creating incoming payments here")
        })?;
    if !node
        .open_payments()
        .allow_creation(peer.ip(), node.now_ms())
    {
        return Err(OpError::new(429, "too_many_requests", "slow down"));
    }
    let mut new = new_incoming(ledger, &authorized, body)?;
    // As grants do: `$host` and `/pay/<slug>` are one wallet address,
    // which `$host` stops naming when the default ledger changes.
    new.wallet_address = urls.wallet_address(ledger, false);
    node.store().prune_incoming(node.now_ms())?;
    let engine = node.engine().ok_or_else(OpError::unavailable)?;
    let row = engine
        .create_incoming_payment(new)
        .await
        .map_err(engine_error)?;
    // The credentials it was made with: the creator's to pay it with.
    let credentials = Credentials {
        destination: row.ilp_address.clone(),
        shared_secret: row.shared_secret,
    };
    Ok(json(
        StatusCode::CREATED,
        &with_methods(urls, &row, Some(credentials)),
    ))
}

fn new_incoming(
    ledger: LedgerId,
    authorized: &Authorized,
    body: CreateIncomingPayment,
) -> Result<NewIncoming, OpError> {
    let asset = ledger.asset();
    let incoming_amount = match body.incoming_amount {
        Some(amount) if !amount.same_asset(asset.code, asset.scale) => {
            return Err(OpError::invalid(
                "the amount is not in the wallet address's asset",
            ));
        }
        Some(amount) if amount.value == 0 => {
            return Err(OpError::invalid("the amount is zero"));
        }
        Some(amount) => Some(amount.value),
        None => None,
    };
    let expires_at_ms = body
        .expires_at
        .as_deref()
        .map(time::parse_rfc3339)
        .transpose()
        .map_err(|_| OpError::invalid("expiresAt is not a date-time"))?;
    let metadata = match &body.metadata {
        Some(metadata) if !resources::metadata_fits(metadata) => {
            return Err(OpError::invalid("the metadata is too large"));
        }
        Some(metadata) => Some(serde_json::to_string(metadata).map_err(OpError::internal)?),
        None => None,
    };
    Ok(NewIncoming {
        ledger,
        wallet_address: body.wallet_address,
        client: Some(authorized.grant.client.clone()),
        incoming_amount,
        expires_at_ms,
        metadata,
    })
}

/// An incoming payment as the specs write it, without its methods.
fn resource(urls: &Urls, row: &IncomingRow) -> IncomingPayment {
    let asset = row.ledger.asset();
    let amount = |value| Amount::new(value, asset.code, asset.scale);
    IncomingPayment {
        id: urls.incoming_payment(row.id),
        wallet_address: urls.rebase(&row.wallet_address),
        completed: row.completed_at_ms.is_some(),
        incoming_amount: row.incoming_amount.map(amount),
        received_amount: amount(row.received_amount),
        expires_at: Some(time::format_rfc3339(row.expires_at_ms)),
        metadata: row
            .metadata
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok()),
        created_at: time::format_rfc3339(row.created_at_ms),
        methods: None,
    }
}

/// With STREAM credentials to pay it, if it takes money.
fn with_methods(
    urls: &Urls,
    row: &IncomingRow,
    credentials: Option<Credentials>,
) -> IncomingPayment {
    IncomingPayment {
        methods: Some(credentials.into_iter().map(ilp_method).collect()),
        ..resource(urls, row)
    }
}

fn find(node: &Node, id: &str) -> Result<IncomingRow, OpError> {
    let not_found = || OpError::not_found("no such incoming payment");
    let id = PaymentId::parse(id).ok_or_else(not_found)?;
    node.store().incoming_payment(id)?.ok_or_else(not_found)
}

/// Whether the grant may read or complete `row`: `read` only reaches what
/// its client created.
fn check_scope(
    scope: Option<Scope>,
    authorized: &Authorized,
    row: &IncomingRow,
) -> Result<(), OpError> {
    let own = row.client.as_deref() == Some(authorized.grant.client.as_str());
    match scope {
        Some(Scope::All) => Ok(()),
        Some(Scope::Own) if own => Ok(()),
        _ => Err(OpError::forbidden(
            "the grant does not reach this incoming payment",
        )),
    }
}

/// `GET /incoming-payments/{id}`: in full with a grant that may read it,
/// with credentials of its own while it takes money, and what anyone may
/// see without one.
async fn get(
    node: &Node,
    urls: &Urls,
    read: &ReadRequest,
    id: &str,
) -> Result<Response<Body>, OpError> {
    let row = find(node, id)?;
    if !read.has_authorization() {
        let public = PublicIncomingPayment {
            received_amount: Some(resource(urls, &row).received_amount),
            auth_server: urls.auth_server(),
        };
        return Ok(json(StatusCode::OK, &public));
    }
    let authorized = authorize(node, read)?;
    let scope = authorized.permits(urls, AccessType::IncomingPayment, Action::Read, row.ledger);
    check_scope(scope, &authorized, &row)?;
    let credentials = fresh_credentials(node, &row).await?;
    Ok(json(StatusCode::OK, &with_methods(urls, &row, credentials)))
}

/// `POST /incoming-payments/{id}/complete`.
async fn complete(
    node: &Node,
    urls: &Urls,
    read: &ReadRequest,
    id: &str,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let row = find(node, id)?;
    let scope = authorized.permits(
        urls,
        AccessType::IncomingPayment,
        Action::Complete,
        row.ledger,
    );
    check_scope(scope, &authorized, &row)?;
    let engine = node.engine().ok_or_else(OpError::unavailable)?;
    let row = engine
        .complete_incoming_payment(row.id)
        .await
        .map_err(engine_error)?;
    Ok(json(StatusCode::OK, &resource(urls, &row)))
}

/// `GET /incoming-payments?wallet-address=…`: a page, newest first.
async fn list(node: &Node, urls: &Urls, read: &ReadRequest) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let query = ListQuery::parse(&read.query)
        .map_err(|_| OpError::invalid("the list query is malformed"))?;
    let ledger = addresses(node)
        .await?
        .ledger_of_url(urls, &query.wallet_address)
        .ok_or_else(|| OpError::invalid("the wallet address is not one of this node"))?;
    let reach = authorized.listed(urls, AccessType::IncomingPayment, ledger);
    if reach.is_empty() {
        return Err(OpError::forbidden(
            "the grant does not allow listing incoming payments",
        ));
    }
    let cursor = query
        .cursor
        .as_deref()
        .map(|cursor| PaymentId::parse(cursor).ok_or_else(|| OpError::invalid("unknown cursor")))
        .transpose()?;
    let page = node
        .store()
        .incoming_page(&IncomingQuery {
            ledger,
            reach: &reach,
            cursor,
            forward: query.direction == Direction::Forward,
            count: u32::from(query.count),
        })?
        .ok_or_else(|| OpError::invalid("unknown cursor"))?;
    let pagination = PageInfo {
        start_cursor: page.rows.first().map(|row| row.id.to_string()),
        end_cursor: page.rows.last().map(|row| row.id.to_string()),
        has_next_page: page.has_older,
        has_previous_page: page.has_newer,
    };
    let result = page.rows.iter().map(|row| resource(urls, row)).collect();
    Ok(json(StatusCode::OK, &Page { pagination, result }))
}
