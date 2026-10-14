//! The owner's Send (D9, M6c): to a payment pointer, a wallet address URL,
//! or a payment request, on this node or another.
//!
//! The owner needs no grant: the node quotes and pays as the owner, with
//! the same quote and payment logic as third-party clients. For a wallet
//! address, the quote creates the incoming payment there first, with a
//! non-interactive grant as the node's `$host` client, expecting what the
//! owner sends; a payment request is paid as it is. Each lookup may fetch
//! from other wallets as the node, so few run at once.

use std::sync::Arc;

use dassie_ledger::LedgerId;
use dassie_payments::amount::Amount;
use dassie_payments::pointer::{self, Recipient};
use dassie_payments::resources::IncomingPayment;
use dassie_payments::time;
use dassie_payments::uri::Uri;

use super::RpcError;
use super::handlers::{Items, Reply};
use super::live::{self, Follow, Source};
use super::payments::outgoing;
use super::proto::{self, request::Method, response, stream_item};
use crate::engine::{Engine, NewIncoming};
use crate::node::Node;
use crate::open_payments::client::{self, ClientError};
use crate::open_payments::spending::{self, NewQuote, Receiver};
use crate::open_payments::{Urls, WalletAddresses};
use crate::store::PaymentId;

/// How long the incoming payment a Send creates accepts money: the quote's
/// five minutes, the payment's ten, and room to spare.
const SEND_RECEIVER_LIFETIME_MS: u64 = 30 * 60 * 1_000;
/// The longest note, in characters, as the wallet counts them.
const MAX_NOTE_CHARS: usize = 256;

/// Answers the Send methods.
pub async fn dispatch(
    node: &Arc<Node>,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let urls = Urls::of(node).ok_or_else(|| RpcError::precondition("the node has no hostname"))?;
    let reply = match method {
        Method::ResolveRecipient(request) => {
            let _permit = lookup_permit(node).await?;
            let found = find(node, &urls, &request.recipient).await?;
            response::Method::ResolveRecipient(proto::ResolveRecipientResponse {
                recipient: Some(found.recipient()),
            })
        }
        Method::QuoteSend(request) => {
            let _permit = lookup_permit(node).await?;
            let quote = quote(node, engine, &urls, request).await?;
            response::Method::QuoteSend(proto::QuoteSendResponse { quote: Some(quote) })
        }
        Method::SendPayment(request) => {
            let id = PaymentId::parse(&request.quote_id)
                .ok_or_else(|| RpcError::invalid("the quote does not exist"))?;
            let new = spending::from_quote(node, id, None, None)?;
            let created = engine.create_outgoing_payment(new).await?;
            response::Method::SendPayment(proto::SendPaymentResponse {
                payment: Some(outgoing(&urls, &created.row)),
            })
        }
        Method::SubscribeOutgoingPayment(request) => {
            return subscribe(node, engine, urls, &request.id, items).await;
        }
        _ => return Err(RpcError::internal("not a Send method")),
    };
    Ok(Reply::Unary(reply))
}

/// Whether a method is one of these.
pub const fn is_send(method: &Method) -> bool {
    matches!(
        method,
        Method::ResolveRecipient(_)
            | Method::QuoteSend(_)
            | Method::SendPayment(_)
            | Method::SubscribeOutgoingPayment(_)
    )
}

async fn lookup_permit(node: &Node) -> Result<tokio::sync::SemaphorePermit<'_>, RpcError> {
    node.open_payments()
        .lookups
        .acquire()
        .await
        .map_err(RpcError::internal)
}

/// What another wallet's failure means to the owner.
fn client_error(error: &ClientError) -> RpcError {
    match error {
        ClientError::Unreachable(_) => RpcError::unavailable(error),
        ClientError::Invalid(_) | ClientError::Refused(_) => RpcError::invalid(error),
        ClientError::Unavailable => RpcError::precondition(error),
    }
}

/// A wallet address, here or elsewhere.
struct Wallet {
    url: String,
    public_name: Option<String>,
    asset_code: String,
    asset_scale: u8,
    /// The ledger, when it is one of this node's.
    own: Option<LedgerId>,
    /// What its wallet says of it, when it is another's.
    remote: Option<dassie_payments::resources::WalletAddress>,
}

/// What the owner's input names.
struct Found {
    wallet: Wallet,
    /// The payment request, when the input named one.
    request: Option<(String, IncomingPayment)>,
}

impl Found {
    fn recipient(&self) -> proto::Recipient {
        let request = self.request.as_ref().map(|(url, payment)| {
            let remaining = payment
                .incoming_amount
                .as_ref()
                .map(|amount| amount.value.saturating_sub(payment.received_amount.value));
            proto::PaymentRequest {
                url: url.clone(),
                remaining,
                expires_at_ms: payment
                    .expires_at
                    .as_deref()
                    .and_then(|at| time::parse_rfc3339(at).ok()),
                description: payment
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("description"))
                    .and_then(|description| description.as_str())
                    .unwrap_or_default()
                    .chars()
                    .take(MAX_NOTE_CHARS)
                    .collect(),
            }
        });
        proto::Recipient {
            wallet_address: self.wallet.url.clone(),
            payment_pointer: pointer::payment_pointer(&self.wallet.url).unwrap_or_default(),
            public_name: self.wallet.public_name.clone().unwrap_or_default(),
            asset_code: self.wallet.asset_code.clone(),
            asset_scale: self.wallet.asset_scale.into(),
            is_own: self.wallet.own.is_some(),
            request,
        }
    }
}

/// The origin of a URL, lowercased.
fn origin_of(url: &str) -> Option<String> {
    Uri::parse(url).ok().map(|uri| uri.origin())
}

/// Reads what `input` names: the wallet address, and the payment request
/// if it is one.
async fn find(node: &Node, urls: &Urls, input: &str) -> Result<Found, RpcError> {
    let recipient = Recipient::parse(input).map_err(|_| {
        RpcError::invalid("that is not a payment pointer, wallet address or payment request")
    })?;
    match recipient {
        Recipient::WalletAddress(url) => Ok(Found {
            wallet: wallet(node, urls, &url).await?,
            request: None,
        }),
        Recipient::IncomingPayment(url) => {
            let payment = spending::incoming_payment(node, urls, &url).await?;
            let wallet = wallet(node, urls, &payment.wallet_address).await?;
            Ok(Found {
                wallet,
                request: Some((url, payment)),
            })
        }
    }
}

/// The wallet address at `url`: one of this node's, read from its ledgers,
/// or another wallet's, read from it.
async fn wallet(node: &Node, urls: &Urls, url: &str) -> Result<Wallet, RpcError> {
    if origin_of(url) == origin_of(urls.origin()) {
        let path = Uri::parse(url)
            .map(|uri| uri.path.to_string())
            .unwrap_or_default();
        let ledger = WalletAddresses::of(node)
            .await
            .map_err(|_| RpcError::unavailable("the ledgers are not running"))?
            .ledger_of_path(&path)
            .ok_or_else(|| RpcError::invalid("this node has no such wallet address"))?;
        let asset = ledger.asset();
        let alias = node.config().alias;
        return Ok(Wallet {
            url: urls.wallet_address(ledger, false),
            public_name: (!alias.as_str().is_empty()).then(|| alias.as_str().to_string()),
            asset_code: asset.code.into(),
            asset_scale: asset.scale,
            own: Some(ledger),
            remote: None,
        });
    }
    let remote = client::resolve(node, url)
        .await
        .map_err(|error| client_error(&error))?;
    Ok(Wallet {
        url: remote.id.clone(),
        public_name: remote.public_name.clone(),
        asset_code: remote.asset_code.clone(),
        asset_scale: remote.asset_scale,
        own: None,
        remote: Some(remote),
    })
}

/// The ledger to pay `wallet` from: the one named, or the default if it is
/// in the wallet's asset, or else the first that is.
async fn ledger_for(node: &Node, named: &str, wallet: &Wallet) -> Result<LedgerId, RpcError> {
    let addresses = WalletAddresses::of(node)
        .await
        .map_err(|_| RpcError::unavailable("the ledgers are not running"))?;
    let fits = |ledger: &LedgerId| {
        let asset = ledger.asset();
        asset.code == wallet.asset_code && asset.scale == wallet.asset_scale
    };
    if !named.is_empty() {
        let ledger = LedgerId::parse(named)
            .filter(|ledger| addresses.ledgers.contains(ledger))
            .ok_or_else(|| RpcError::precondition("the ledger is not active"))?;
        return if fits(&ledger) {
            Ok(ledger)
        } else {
            Err(RpcError::invalid("payments across assets come with M7"))
        };
    }
    addresses
        .default
        .filter(fits)
        .or_else(|| addresses.ledgers.iter().copied().find(fits))
        .ok_or_else(|| {
            RpcError::invalid(format!(
                "no active ledger is in {}; payments across assets come with M7",
                wallet.asset_code
            ))
        })
}

/// Quotes a payment, creating the incoming payment at a wallet address
/// first.
async fn quote(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    request: proto::QuoteSendRequest,
) -> Result<proto::SendQuote, RpcError> {
    if request.note.chars().count() > MAX_NOTE_CHARS {
        return Err(RpcError::invalid("the note is too long"));
    }
    let found = find(node, urls, &request.recipient).await?;
    let ledger = ledger_for(node, &request.ledger, &found.wallet).await?;
    let (receiver_url, receiver) = match &found.request {
        Some((url, payment)) => {
            if !request.note.is_empty() {
                return Err(RpcError::invalid(
                    "a payment request has its own description",
                ));
            }
            (url.clone(), spending::receiver_of(node, payment.clone())?)
        }
        None => {
            let amount = request
                .amount
                .ok_or_else(|| RpcError::invalid("name an amount"))?;
            if amount == 0 || amount > dassie_ledger::MAX_TOTAL {
                return Err(RpcError::invalid("the amount is out of range"));
            }
            create_receiver(node, engine, urls, &found.wallet, amount, &request.note).await?
        }
    };
    let wallet = &found.wallet;
    let receive = request
        .amount
        .map(|value| Amount::new(value, &wallet.asset_code, wallet.asset_scale));
    let (debit, receive) = spending::amounts(ledger, &receiver, None, receive.as_ref())?;
    let row = spending::insert_quote(
        node,
        urls,
        NewQuote {
            ledger,
            client: None,
            receiver_url,
            receiver,
            debit,
            receive,
            recipient: Some(wallet.url.clone()),
        },
    )?;
    let asset = ledger.asset();
    Ok(proto::SendQuote {
        id: row.id.to_string(),
        ledger: ledger.as_str().into(),
        asset_code: asset.code.into(),
        asset_scale: asset.scale.into(),
        debit_amount: row.debit_amount,
        receive_amount: Some(proto::OpenPaymentsAmount {
            value: row.receive_amount.value,
            asset_code: row.receive_amount.asset_code.clone(),
            asset_scale: row.receive_amount.asset_scale.into(),
        }),
        receiver: row.receiver,
        expires_at_ms: row.expires_at_ms,
        recipient: Some(found.recipient()),
    })
}

/// Creates the incoming payment a Send pays, at `wallet`, expecting
/// `amount`, with the note as its description.
async fn create_receiver(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    wallet: &Wallet,
    amount: u64,
    note: &str,
) -> Result<(String, Receiver), RpcError> {
    let expires_at_ms = node.now_ms() + SEND_RECEIVER_LIFETIME_MS;
    let metadata = (!note.is_empty()).then(|| {
        let mut metadata = serde_json::Map::new();
        metadata.insert("description".into(), note.into());
        metadata
    });
    if let Some(ledger) = wallet.own {
        let row = engine
            .create_incoming_payment(NewIncoming {
                ledger,
                wallet_address: wallet.url.clone(),
                client: None,
                incoming_amount: Some(amount),
                expires_at_ms: Some(expires_at_ms),
                metadata: metadata.map(|metadata| serde_json::Value::Object(metadata).to_string()),
            })
            .await?;
        let payment = spending::local_receiver(node, urls, &row.id.to_string())?;
        return Ok((
            urls.incoming_payment(row.id),
            spending::receiver_of(node, payment)?,
        ));
    }
    let remote = wallet
        .remote
        .as_ref()
        .ok_or_else(|| RpcError::internal("a wallet address that is neither"))?;
    let payment = client::create_incoming(node, remote, Some(amount), expires_at_ms, metadata)
        .await
        .map_err(|error| client_error(&error))?;
    let url = payment.id.clone();
    Ok((url, spending::receiver_of(node, payment)?))
}

/// One outgoing payment, whole, each time it changes.
async fn subscribe(
    node: &Arc<Node>,
    engine: &Engine,
    urls: Urls,
    id: &str,
    items: &Items,
) -> Result<Reply, RpcError> {
    let not_found = || RpcError::not_found("no such outgoing payment");
    let id = PaymentId::parse(id).ok_or_else(not_found)?;
    let _permit = node
        .live_stream_permit()
        .ok_or_else(|| RpcError::exhausted("too many live streams"))?;
    let source = Source {
        versions: engine.versions(),
        ended: engine.generation().clone(),
    };
    let store_node = Arc::clone(node);
    let urls = &urls;
    live::values(
        source,
        items,
        Follow::Payments,
        || {
            let node = Arc::clone(&store_node);
            async move {
                let row = node
                    .store()
                    .outgoing_payment(id)
                    .map_err(RpcError::internal)?
                    .ok_or_else(not_found)?;
                Ok(outgoing(urls, &row))
            }
        },
        stream_item::Method::SubscribeOutgoingPayment,
    )
    .await
}
