//! The resource server's spending: quotes, outgoing payments, and what a
//! grant spent (D9).
//!
//! The quote and payment logic is shared with the owner's Send, in
//! [`crate::open_payments::spending`]; here are the grant's checks around
//! it. The engine reserves a payment against the grant's limits in the
//! window it is made in, and sends it at once.

use std::net::SocketAddr;

use dassie_ledger::LedgerId;
use dassie_payments::amount::Amount;
use dassie_payments::gnap::{AccessType, Action, Limits, Scope};
use dassie_payments::resources::{
    self, CreateOutgoingPayment, CreateQuote, Direction, ListQuery, OutgoingPayment,
    OutgoingPaymentGrant, Page, PageInfo, Quote,
};
use dassie_payments::time;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use super::incoming::engine_error;
use super::{ReadRequest, error, json};
use crate::http::response::Body;
use crate::node::Node;
use crate::open_payments::auth::{self, Authorized, OpError};
use crate::open_payments::spending::{self, NewQuote};
use crate::open_payments::{Urls, WalletAddresses};
use crate::store::{ForeignAmount, IncomingQuery, OutgoingRow, OutgoingState, PaymentId, QuoteRow};

/// Routes `/quotes[/{id}]`, `/outgoing-payments[/{id}]` and
/// `/outgoing-payment-grant`.
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
    let segments: Vec<String> = read.path.split('/').skip(1).map(String::from).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let result = match (&method, segments.as_slice()) {
        (&Method::POST, ["quotes"]) => create_quote(node, peer, urls, &read).await,
        (&Method::GET | &Method::HEAD, ["quotes", id]) => get_quote(node, urls, &read, id).await,
        (&Method::POST, ["outgoing-payments"]) => create_outgoing(node, peer, urls, &read).await,
        (&Method::GET, ["outgoing-payments"]) => list_outgoing(node, urls, &read).await,
        (&Method::GET | &Method::HEAD, ["outgoing-payments", id]) => {
            get_outgoing(node, urls, &read, id).await
        }
        (&Method::GET | &Method::HEAD, ["outgoing-payment-grant"]) => {
            grant_spent(node, urls, &read).await
        }
        _ => Err(OpError::not_found("Not Found")),
    };
    result.unwrap_or_else(|failure| error(&failure, Some(urls)))
}

fn authorize(node: &Node, read: &ReadRequest) -> Result<Authorized, OpError> {
    let fields = read.field_refs();
    auth::authorize(node, &read.signed(&fields))
}

/// The ledger of one of the node's wallet addresses.
async fn ledger_of(node: &Node, urls: &Urls, wallet_address: &str) -> Result<LedgerId, OpError> {
    WalletAddresses::of(node)
        .await
        .map_err(|_| OpError::unavailable())?
        .ledger_of_url(urls, wallet_address)
        .ok_or_else(|| OpError::invalid("the wallet address is not one of this node"))
}

/// `POST /quotes`.
async fn create_quote(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    read: &ReadRequest,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let body: CreateQuote = serde_json::from_slice(&read.body)
        .map_err(|_| OpError::invalid("the quote request is malformed"))?;
    if body.method != "ilp" {
        return Err(OpError::invalid("the only method is ilp"));
    }
    let ledger = ledger_of(node, urls, &body.wallet_address).await?;
    authorized
        .permits(urls, AccessType::Quote, Action::Create, ledger)
        .ok_or_else(|| OpError::forbidden("the grant does not allow quotes here"))?;
    if !node
        .open_payments()
        .allow_creation(peer.ip(), node.now_ms())
    {
        return Err(OpError::new(429, "too_many_requests", "slow down"));
    }
    let receiver = spending::receiver(node, urls, &body.receiver).await?;
    let rate = spending::quote_rate(node, ledger, &receiver).await?;
    let (debit, receive) = spending::amounts(
        ledger,
        &receiver,
        rate,
        body.debit_amount.as_ref(),
        body.receive_amount.as_ref(),
    )?;
    let row = spending::insert_quote(
        node,
        urls,
        NewQuote {
            ledger,
            client: Some(authorized.grant.client.clone()),
            receiver_url: body.receiver,
            receiver,
            debit,
            receive,
            recipient: None,
            fixed_debit: body.debit_amount.is_some(),
        },
    )?;
    Ok(json(StatusCode::CREATED, &quote(urls, &row)))
}

fn quote(urls: &Urls, row: &QuoteRow) -> Quote {
    let asset = row.ledger.asset();
    Quote {
        id: format!("{}/quotes/{}", urls.origin(), row.id),
        wallet_address: row.wallet_address.clone(),
        receiver: row.receiver.clone(),
        receive_amount: foreign(&row.receive_amount),
        debit_amount: Amount::new(row.debit_amount, asset.code, asset.scale),
        method: "ilp".into(),
        expires_at: Some(time::format_rfc3339(row.expires_at_ms)),
        created_at: time::format_rfc3339(row.created_at_ms),
    }
}

fn foreign(amount: &ForeignAmount) -> Amount {
    Amount::new(amount.value, &amount.asset_code, amount.asset_scale)
}

/// Whether `scope` reaches a resource `client` created.
fn reaches(scope: Option<Scope>, authorized: &Authorized, client: Option<&str>) -> bool {
    match scope {
        Some(Scope::All) => true,
        Some(Scope::Own) => client == Some(authorized.grant.client.as_str()),
        None => false,
    }
}

/// `GET /quotes/{id}`.
async fn get_quote(
    node: &Node,
    urls: &Urls,
    read: &ReadRequest,
    id: &str,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let not_found = || OpError::not_found("no such quote");
    let id = PaymentId::parse(id).ok_or_else(not_found)?;
    let row = node.store().quote(id)?.ok_or_else(not_found)?;
    let scope = authorized.permits(urls, AccessType::Quote, Action::Read, row.ledger);
    if !reaches(scope, &authorized, row.client.as_deref()) {
        return Err(OpError::forbidden("the grant does not reach this quote"));
    }
    Ok(json(StatusCode::OK, &quote(urls, &row)))
}

/// The limits of the grant's outgoing-payment access for the wallet
/// address of `ledger`: none if it names none.
fn limits_of(authorized: &Authorized, urls: &Urls, ledger: LedgerId) -> Limits {
    let wallet_address = urls.wallet_address(ledger, false);
    authorized
        .access
        .iter()
        .filter(|item| item.kind == AccessType::OutgoingPayment)
        .find(|item| item.identifier.as_deref() == Some(wallet_address.as_str()))
        .and_then(|item| item.limits.clone())
        .unwrap_or(Limits {
            receiver: None,
            interval: None,
            debit_amount: None,
            receive_amount: None,
        })
}

/// `POST /outgoing-payments`: from a quote of the client's, or from an
/// incoming payment with the amount to debit.
async fn create_outgoing(
    node: &Node,
    peer: SocketAddr,
    urls: &Urls,
    read: &ReadRequest,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let body: CreateOutgoingPayment = serde_json::from_slice(&read.body)
        .map_err(|_| OpError::invalid("the outgoing payment is malformed"))?;
    let ledger = ledger_of(node, urls, &body.wallet_address).await?;
    authorized
        .permits(urls, AccessType::OutgoingPayment, Action::Create, ledger)
        .ok_or_else(|| OpError::forbidden("the grant does not allow payments from here"))?;
    // It may read another wallet, and moves money: as limited as quotes.
    if !node
        .open_payments()
        .allow_creation(peer.ip(), node.now_ms())
    {
        return Err(OpError::new(429, "too_many_requests", "slow down"));
    }
    let metadata = match &body.metadata {
        Some(metadata) if !resources::metadata_fits(metadata) => {
            return Err(OpError::invalid("the metadata is too large"));
        }
        Some(metadata) => Some(serde_json::to_string(metadata).map_err(OpError::internal)?),
        None => None,
    };
    let mut new = match (&body.quote_id, &body.incoming_payment, &body.debit_amount) {
        (Some(quote_id), None, None) => {
            let id = quote_id
                .strip_prefix(&format!("{}/quotes/", urls.origin()))
                .and_then(PaymentId::parse)
                .ok_or_else(|| OpError::invalid("the quote does not exist"))?;
            spending::from_quote(node, id, Some(&authorized.grant.client), Some(ledger))?
        }
        (None, Some(incoming), Some(debit)) => {
            spending::from_incoming(node, urls, ledger, incoming, debit).await?
        }
        _ => {
            return Err(OpError::invalid(
                "name a quote, or an incoming payment and the amount to debit",
            ));
        }
    };
    new.client = Some(authorized.grant.client.clone());
    new.grant = Some((authorized.grant.id, limits_of(&authorized, urls, ledger)));
    new.metadata = metadata;
    let engine = node.engine().ok_or_else(OpError::unavailable)?;
    let created = engine
        .create_outgoing_payment(new)
        .await
        .map_err(engine_error)?;
    let mut payment = outgoing(urls, &created.row);
    if let Some((debit, receive)) = created.grant_spent {
        let asset = ledger.asset();
        payment.grant_spent_debit_amount = Some(Amount::new(debit, asset.code, asset.scale));
        let receive_asset = &created.row.receive_amount;
        payment.grant_spent_receive_amount = Some(Amount::new(
            receive,
            &receive_asset.asset_code,
            receive_asset.asset_scale,
        ));
    }
    Ok(json(StatusCode::CREATED, &payment))
}

/// An outgoing payment as the specs write it.
fn outgoing(urls: &Urls, row: &OutgoingRow) -> OutgoingPayment {
    let asset = row.ledger.asset();
    let amount = |value| Amount::new(value, asset.code, asset.scale);
    OutgoingPayment {
        id: format!("{}/outgoing-payments/{}", urls.origin(), row.id),
        wallet_address: row.wallet_address.clone(),
        quote_id: row
            .quote_id
            .map(|quote| format!("{}/quotes/{quote}", urls.origin())),
        failed: row.state == OutgoingState::Failed,
        receiver: row.receiver.clone(),
        receive_amount: foreign(&row.receive_amount),
        debit_amount: amount(row.debit_amount),
        sent_amount: amount(row.sent_amount),
        metadata: row
            .metadata
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok()),
        created_at: time::format_rfc3339(row.created_at_ms),
        grant_spent_debit_amount: None,
        grant_spent_receive_amount: None,
    }
}

/// `GET /outgoing-payments/{id}`.
async fn get_outgoing(
    node: &Node,
    urls: &Urls,
    read: &ReadRequest,
    id: &str,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let not_found = || OpError::not_found("no such outgoing payment");
    let id = PaymentId::parse(id).ok_or_else(not_found)?;
    let row = node.store().outgoing_payment(id)?.ok_or_else(not_found)?;
    let scope = authorized.permits(urls, AccessType::OutgoingPayment, Action::Read, row.ledger);
    if !reaches(scope, &authorized, row.client.as_deref()) {
        return Err(OpError::forbidden(
            "the grant does not reach this outgoing payment",
        ));
    }
    Ok(json(StatusCode::OK, &outgoing(urls, &row)))
}

/// `GET /outgoing-payments?wallet-address=…`: a page, newest first.
async fn list_outgoing(
    node: &Node,
    urls: &Urls,
    read: &ReadRequest,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let query = ListQuery::parse(&read.query)
        .map_err(|_| OpError::invalid("the list query is malformed"))?;
    let ledger = ledger_of(node, urls, &query.wallet_address).await?;
    let scope = authorized
        .permits(urls, AccessType::OutgoingPayment, Action::List, ledger)
        .ok_or_else(|| OpError::forbidden("the grant does not allow listing outgoing payments"))?;
    let cursor = query
        .cursor
        .as_deref()
        .map(|cursor| PaymentId::parse(cursor).ok_or_else(|| OpError::invalid("unknown cursor")))
        .transpose()?;
    let client = (scope == Scope::Own).then_some(authorized.grant.client.as_str());
    let page = node
        .store()
        .outgoing_page(&IncomingQuery {
            ledger,
            client,
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
    let result = page.rows.iter().map(|row| outgoing(urls, row)).collect();
    Ok(json(StatusCode::OK, &Page { pagination, result }))
}

/// `GET /outgoing-payment-grant`: what the token's grant spent in the
/// current window of its interval; nothing spent is null.
async fn grant_spent(
    node: &Node,
    urls: &Urls,
    read: &ReadRequest,
) -> Result<Response<Body>, OpError> {
    let authorized = authorize(node, read)?;
    let item = authorized
        .access
        .iter()
        .find(|item| item.kind == AccessType::OutgoingPayment)
        .ok_or_else(|| OpError::forbidden("the grant is not for outgoing payments"))?;
    let ledger = ledger_of(node, urls, item.identifier.as_deref().unwrap_or_default()).await?;
    let limits = limits_of(&authorized, urls, ledger);
    let spent = match limits.window(node.now_ms()) {
        Some((start, _)) => node.store().grant_spend(authorized.grant.id, start)?,
        None => (0, 0),
    };
    let asset = ledger.asset();
    let receive_asset = limits
        .receive_amount
        .as_ref()
        .map_or((asset.code.to_string(), asset.scale), |limit| {
            (limit.asset_code.clone(), limit.asset_scale)
        });
    let body = OutgoingPaymentGrant {
        spent_debit_amount: (spent.0 > 0).then(|| Amount::new(spent.0, asset.code, asset.scale)),
        spent_receive_amount: (spent.1 > 0)
            .then(|| Amount::new(spent.1, &receive_asset.0, receive_asset.1)),
    };
    Ok(json(StatusCode::OK, &body))
}
