//! Open Payments for the owner (D9): the wallet addresses, the default
//! ledger, and incoming payments.

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_ledger::LedgerId;

use super::RpcError;
use super::handlers::{Items, Reply};
use super::live::{self, Follow, Source};
use super::proto::{self, request::Method, response, stream_item};
use crate::engine::{Engine, EngineError, NewIncoming};
use crate::node::Node;
use crate::open_payments::{AddressError, Urls, WalletAddresses};
use crate::store::{IncomingRow, PaymentId};

/// Page sizes for incoming payments.
const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 1_000;

/// The longest description of an incoming payment the owner creates, in
/// characters, as the wallet counts them.
const MAX_DESCRIPTION_CHARS: usize = 256;

/// Answers the Open Payments methods.
pub async fn dispatch(
    node: &Arc<Node>,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let urls = Urls::of(node).ok_or_else(|| RpcError::precondition("the node has no hostname"))?;
    let reply = match method {
        Method::ListWalletAddresses(_) => {
            let addresses = addresses(node).await?;
            response::Method::ListWalletAddresses(proto::ListWalletAddressesResponse {
                wallet_addresses: wallet_addresses(&urls, &addresses),
            })
        }
        Method::SetDefaultLedger(request) => set_default_ledger(node, &urls, &request).await?,
        Method::CreateIncomingPayment(request) => {
            let row = create(node, engine, &urls, request).await?;
            response::Method::CreateIncomingPayment(proto::CreateIncomingPaymentResponse {
                payment: Some(incoming(&urls, &row)),
            })
        }
        Method::CompleteIncomingPayment(request) => {
            let id = PaymentId::parse(&request.id)
                .ok_or_else(|| RpcError::not_found("no such incoming payment"))?;
            let row = engine.complete_incoming_payment(id).await?;
            response::Method::CompleteIncomingPayment(proto::CompleteIncomingPaymentResponse {
                payment: Some(incoming(&urls, &row)),
            })
        }
        Method::ListIncomingPayments(request) => {
            let (_, rows) = node
                .store()
                .incoming_latest(limit(request.limit))
                .map_err(RpcError::internal)?;
            response::Method::ListIncomingPayments(proto::ListIncomingPaymentsResponse {
                payments: rows.iter().map(|row| incoming(&urls, row)).collect(),
            })
        }
        Method::SubscribeIncomingPayments(request) => {
            return subscribe(node, engine, urls, limit(request.limit), items).await;
        }
        _ => return Err(RpcError::internal("not a payments method")),
    };
    Ok(Reply::Unary(reply))
}

/// Whether a method is one of these.
pub const fn is_payments(method: &Method) -> bool {
    matches!(
        method,
        Method::ListWalletAddresses(_)
            | Method::SetDefaultLedger(_)
            | Method::CreateIncomingPayment(_)
            | Method::CompleteIncomingPayment(_)
            | Method::ListIncomingPayments(_)
            | Method::SubscribeIncomingPayments(_)
    )
}

fn limit(requested: u32) -> u32 {
    match requested {
        0 => DEFAULT_LIMIT,
        n => n.min(MAX_LIMIT),
    }
}

async fn addresses(node: &Node) -> Result<WalletAddresses, RpcError> {
    WalletAddresses::of(node)
        .await
        .map_err(|error| match error {
            AddressError::Unavailable => RpcError::unavailable(EngineError::Unavailable),
            AddressError::Store(error) => RpcError::internal(error),
        })
}

fn wallet_addresses(urls: &Urls, addresses: &WalletAddresses) -> Vec<proto::WalletAddress> {
    addresses
        .ledgers
        .iter()
        .map(|&ledger| {
            let is_default = addresses.is_default(ledger);
            let asset = ledger.asset();
            proto::WalletAddress {
                url: urls.wallet_address(ledger, is_default),
                payment_pointer: urls.payment_pointer(ledger, is_default),
                ledger: ledger.as_str().into(),
                asset_code: asset.code.into(),
                asset_scale: asset.scale.into(),
                is_default,
            }
        })
        .collect()
}

async fn set_default_ledger(
    node: &Node,
    urls: &Urls,
    request: &proto::SetDefaultLedgerRequest,
) -> Result<response::Method, RpcError> {
    let chosen = if request.ledger.is_empty() {
        None
    } else {
        let ledger = LedgerId::parse(&request.ledger)
            .ok_or_else(|| RpcError::not_found("no such ledger"))?;
        if !addresses(node).await?.ledgers.contains(&ledger) {
            return Err(RpcError::not_found("the ledger is not active"));
        }
        Some(ledger)
    };
    node.store()
        .set_default_ledger(chosen)
        .map_err(RpcError::internal)?;
    let addresses = addresses(node).await?;
    Ok(response::Method::SetDefaultLedger(
        proto::SetDefaultLedgerResponse {
            wallet_addresses: wallet_addresses(urls, &addresses),
        },
    ))
}

async fn create(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    request: proto::CreateIncomingPaymentRequest,
) -> Result<IncomingRow, RpcError> {
    let addresses = addresses(node).await?;
    let ledger = if request.ledger.is_empty() {
        addresses.default
    } else {
        LedgerId::parse(&request.ledger).filter(|ledger| addresses.ledgers.contains(ledger))
    }
    .ok_or_else(|| RpcError::precondition("the ledger is not active"))?;
    if request.description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(RpcError::invalid("the description is too long"));
    }
    let metadata = (!request.description.is_empty())
        .then(|| serde_json::json!({ "description": request.description }).to_string());
    let new = NewIncoming {
        ledger,
        wallet_address: urls.wallet_address(ledger, false),
        client: None,
        incoming_amount: request.incoming_amount,
        expires_at_ms: (request.expires_at_ms != 0).then_some(request.expires_at_ms),
        metadata,
    };
    node.store()
        .prune_incoming(node.now_ms())
        .map_err(RpcError::internal)?;
    Ok(engine.create_incoming_payment(new).await?)
}

/// An incoming payment, for the owner.
pub fn incoming(urls: &Urls, row: &IncomingRow) -> proto::IncomingPayment {
    let asset = row.ledger.asset();
    proto::IncomingPayment {
        id: row.id.to_string(),
        url: urls.incoming_payment(row.id),
        wallet_address: row.wallet_address.clone(),
        ledger: row.ledger.as_str().into(),
        asset_code: asset.code.into(),
        asset_scale: asset.scale.into(),
        incoming_amount: row.incoming_amount,
        received_amount: row.received_amount,
        client: row.client.clone().unwrap_or_default(),
        metadata_json: row.metadata.clone().unwrap_or_default(),
        created_at_ms: row.created_at_ms,
        expires_at_ms: row.expires_at_ms,
        completed_at_ms: row.completed_at_ms,
        destination_account: row.ilp_address.clone(),
        shared_secret: row.shared_secret.to_vec(),
    }
}

/// The newest incoming payments, then their changes, until the call ends
/// or the engine stops.
async fn subscribe(
    node: &Arc<Node>,
    engine: &Engine,
    urls: Urls,
    limit: u32,
    items: &Items,
) -> Result<Reply, RpcError> {
    let _permit = node
        .live_stream_permit()
        .ok_or_else(|| RpcError::exhausted("too many live streams"))?;
    let source = Source {
        versions: engine.versions(),
        ended: engine.generation().clone(),
    };
    let store_node = Arc::clone(node);
    let urls = &urls;
    live::table(
        source,
        items,
        Follow::Payments,
        || {
            let node = Arc::clone(&store_node);
            async move {
                let (_, rows) = node
                    .store()
                    .incoming_latest(limit)
                    .map_err(RpcError::internal)?;
                Ok(rows
                    .iter()
                    .map(|row| (row.id, incoming(urls, row)))
                    .collect::<BTreeMap<_, _>>())
            }
        },
        |reset, upserted, removed: Vec<PaymentId>| {
            stream_item::Method::SubscribeIncomingPayments(proto::IncomingPaymentEvent {
                reset,
                upserted,
                removed: removed.iter().map(PaymentId::to_string).collect(),
            })
        },
    )
    .await
}
