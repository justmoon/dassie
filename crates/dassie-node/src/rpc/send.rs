//! The owner's Send (spec/decisions/open-payments.md,
//! spec/areas/payments.md#the-owners-send): to a payment pointer, a wallet
//! address URL, a payment request, or STREAM credentials, on this node or
//! another.
//!
//! The owner needs no grant: the node quotes and pays as the owner, with
//! the same quote and payment logic as third-party clients. For a wallet
//! address, the quote creates the incoming payment there first, with a
//! non-interactive grant as the node's `$host` client, expecting what the
//! owner sends; a payment request is paid as it is, and so are STREAM
//! credentials. Each lookup may fetch from other wallets as the node, so few
//! run at once.

use std::sync::Arc;
use std::time::Duration;

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
use super::proto::quote_send_request::Amount as Amount_;
use super::proto::{self, request::Method, response, stream_item};
use crate::engine::{Engine, NewIncoming, Prober};
use crate::node::Node;
use crate::open_payments::client::{self, ClientError};
use crate::open_payments::spending::{self, NewQuote, Receiver};
use crate::open_payments::{Urls, WELL_KNOWN_PATH, WalletAddresses};

use crate::store::{PaymentId, QuoteRow};

/// How long the incoming payment a Send creates accepts money: the quote's
/// five minutes, the payment's ten, and room to spare.
const SEND_RECEIVER_LIFETIME_MS: u64 = 30 * 60 * 1_000;
/// The longest note, in characters, as the wallet counts them.
const MAX_NOTE_CHARS: usize = 256;
/// How long a lookup waits for its turn.
const LOOKUP_WAIT: Duration = Duration::from_secs(30);

/// Answers the Send methods.
pub async fn dispatch(
    node: &Arc<Node>,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let urls = Urls::of(node).ok_or_else(|| RpcError::precondition("the node has no name yet"))?;
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

/// Waits for a turn to look up, for a while.
async fn lookup_permit(node: &Node) -> Result<tokio::sync::SemaphorePermit<'_>, RpcError> {
    match tokio::time::timeout(LOOKUP_WAIT, node.open_payments().lookups.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(error)) => Err(RpcError::internal(error)),
        Err(_) => Err(RpcError::exhausted(
            "too many lookups at once; try again in a moment",
        )),
    }
}

/// Text from another wallet, as the owner reads it: without control or
/// direction-changing characters, which could make it read as something
/// else beside the payment pointer.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                )
        })
        .collect()
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
                    .map(|description| plain(description).chars().take(MAX_NOTE_CHARS).collect())
                    .unwrap_or_default(),
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
            // Paid in full, expired, or not payable over ILP: said now.
            spending::receiver_of(node, payment.clone())?;
            let wallet = wallet(node, urls, &payment.wallet_address).await?;
            // A request speaks only for its own wallet: one here for one of
            // this node's, one elsewhere for a wallet whose resource
            // server is where the request is. Else it could name anyone,
            // the owner too, and be paid as them.
            let here = urls.own_path(&url).is_some();
            let bound = match &wallet.remote {
                None => here,
                Some(remote) => !here && origin_of(&remote.resource_server) == origin_of(&url),
            };
            if !bound {
                return Err(RpcError::invalid(
                    "the payment request names a wallet address that is not its own",
                ));
            }
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
    if let Some(path) = urls.own_path(url) {
        let ledger = WalletAddresses::of(node)
            .await
            .map_err(|_| RpcError::unavailable("the ledgers are not running"))?
            .ledger_of_path(&path)
            .ok_or_else(|| RpcError::invalid("this node has no such wallet address"))?;
        let asset = ledger.asset();
        let alias = node.config().alias;
        // `$host` stays `$host`, as the owner wrote it.
        return Ok(Wallet {
            url: urls.wallet_address(ledger, path == WELL_KNOWN_PATH),
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
        public_name: remote.public_name.as_deref().map(plain),
        asset_code: remote.asset_code.clone(),
        asset_scale: remote.asset_scale,
        own: None,
        remote: Some(remote),
    })
}

/// The ledger to pay the receiver at `destination` from: the one the route
/// to it starts on, since the node's own endpoints never convert. One
/// named must be it.
async fn ledger_for(engine: &Engine, named: &str, destination: &str) -> Result<LedgerId, RpcError> {
    let (ledger, _) = engine
        .route_of(destination.to_string())
        .await?
        .ok_or_else(|| RpcError::precondition("there is no route to the recipient"))?;
    if !named.is_empty() && named != ledger.as_str() {
        return Err(RpcError::invalid(format!(
            "pay from {ledger}: the route to the recipient starts there"
        )));
    }
    Ok(ledger)
}

/// Quotes a payment, creating the incoming payment at a wallet address
/// first.
async fn quote(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    mut request: proto::QuoteSendRequest,
) -> Result<proto::SendQuote, RpcError> {
    if let Some(credentials) = request.credentials.take() {
        return quote_credentials(node, engine, urls, request, credentials).await;
    }
    if request.note.chars().count() > MAX_NOTE_CHARS {
        return Err(RpcError::invalid("the note is too long"));
    }
    let found = find(node, urls, &request.recipient).await?;
    let (debit_amount, receive_amount) = fixed_amounts(request.amount)?;
    let (receiver_url, receiver, created) = match &found.request {
        Some((url, payment)) => {
            if !request.note.is_empty() {
                return Err(RpcError::invalid(
                    "a payment request has its own description",
                ));
            }
            (
                url.clone(),
                spending::receiver_of(node, payment.clone())?,
                None,
            )
        }
        None => {
            if debit_amount.is_none() && receive_amount.is_none() {
                return Err(RpcError::invalid("name an amount"));
            }
            // What is sent buys what it buys: the incoming payment takes
            // any amount then.
            create_receiver(
                node,
                engine,
                urls,
                &found.wallet,
                receive_amount,
                &request.note,
            )
            .await?
        }
    };
    let quoted = price(
        node,
        engine,
        urls,
        &request.ledger,
        (debit_amount, receive_amount),
        &found,
        receiver_url,
        receiver,
    )
    .await;
    // An incoming payment made here for a quote that failed takes nothing
    // more. One made on another wallet lapses with its expiry.
    if quoted.is_err()
        && let Some(id) = created
        && let Err(error) = engine.complete_incoming_payment(id).await
    {
        tracing::debug!(target: crate::log::target::PAYMENT, %error, "closing an unquoted incoming payment");
    }
    quoted
}

/// What paying `receiver` debits and delivers, as a quote kept for the
/// owner.
#[allow(clippy::too_many_arguments)]
async fn price(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    named: &str,
    (debit_amount, receive_amount): (Option<u64>, Option<u64>),
    found: &Found,
    receiver_url: String,
    receiver: Receiver,
) -> Result<proto::SendQuote, RpcError> {
    let ledger = ledger_for(engine, named, &receiver.destination).await?;
    let rate = spending::quote_rate(node, ledger, &receiver, Prober::Owner).await?;
    let wallet = &found.wallet;
    let asset = ledger.asset();
    let debit = debit_amount.map(|value| Amount::new(value, asset.code, asset.scale));
    let receive =
        receive_amount.map(|value| Amount::new(value, &wallet.asset_code, wallet.asset_scale));
    let (debit, receive) =
        spending::amounts(ledger, &receiver, rate, debit.as_ref(), receive.as_ref())?;
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
            fixed_debit: debit_amount.is_some(),
        },
    )?;
    Ok(send_quote(row, Some(found.recipient())))
}

/// Quotes a payment to STREAM credentials
/// (spec/areas/payments.md#paying-stream-credentials): there is no
/// incoming payment to read, or to create with a note, so
/// nothing says what it accepts, and its asset is the one the owner names
/// or the receiver says in the probe. An amount to send is always taken;
/// one to deliver only in an asset the owner named, as the incoming payment
/// the credentials came with says, since the owner cannot know what the
/// receiver will say.
async fn quote_credentials(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    request: proto::QuoteSendRequest,
    credentials: proto::StreamCredentials,
) -> Result<proto::SendQuote, RpcError> {
    if !request.recipient.is_empty() {
        return Err(RpcError::invalid(
            "name a recipient or STREAM credentials, not both",
        ));
    }
    if !request.note.is_empty() {
        return Err(RpcError::invalid("STREAM credentials take no note"));
    }
    let shared_secret =
        spending::check_credentials(&credentials.destination_account, &credentials.shared_secret)?;
    let named = if credentials.asset_code.is_empty() {
        None
    } else {
        let scale = u8::try_from(credentials.asset_scale)
            .map_err(|_| RpcError::invalid("the asset scale is out of range"))?;
        if !spending::is_asset_code(&credentials.asset_code) {
            return Err(RpcError::invalid("the asset code is invalid"));
        }
        Some((credentials.asset_code, scale))
    };
    let (debit_amount, receive_amount) = fixed_amounts(request.amount)?;
    match (debit_amount, receive_amount) {
        (None, None) => return Err(RpcError::invalid("name an amount")),
        (None, Some(_)) if named.is_none() => {
            return Err(RpcError::invalid(
                "name the receiver's asset to fix what it receives",
            ));
        }
        _ => {}
    }
    let destination = credentials.destination_account;
    let ledger = ledger_for(engine, &request.ledger, &destination).await?;
    let (rate, said) =
        spending::measure(node, ledger, (&destination, shared_secret), Prober::Owner).await?;
    let receiver = spending::credentials_receiver(destination, shared_secret, named, said)?;
    let asset = ledger.asset();
    let debit = debit_amount.map(|value| Amount::new(value, asset.code, asset.scale));
    let receive =
        receive_amount.map(|value| Amount::new(value, &receiver.asset_code, receiver.asset_scale));
    let (debit, receive) =
        spending::amounts(ledger, &receiver, rate, debit.as_ref(), receive.as_ref())?;
    let row = spending::insert_quote(
        node,
        urls,
        NewQuote {
            ledger,
            client: None,
            // No incoming payment's URL: the address stands for it, which
            // marks the quote as one to credentials.
            receiver_url: receiver.destination.clone(),
            receiver,
            debit,
            receive,
            recipient: None,
            fixed_debit: debit_amount.is_some(),
        },
    )?;
    Ok(send_quote(row, None))
}

/// A quote as the owner reads it.
fn send_quote(row: QuoteRow, recipient: Option<proto::Recipient>) -> proto::SendQuote {
    let asset = row.ledger.asset();
    proto::SendQuote {
        id: row.id.to_string(),
        ledger: row.ledger.as_str().into(),
        asset_code: asset.code.into(),
        asset_scale: asset.scale.into(),
        debit_amount: row.debit_amount,
        receive_amount: Some(proto::OpenPaymentsAmount {
            value: row.receive_amount.value,
            asset_code: row.receive_amount.asset_code,
            asset_scale: row.receive_amount.asset_scale.into(),
        }),
        receiver: row.receiver,
        expires_at_ms: row.expires_at_ms,
        recipient,
    }
}

/// The amount to send and the amount to deliver, at most one of them.
fn fixed_amounts(amount: Option<Amount_>) -> Result<(Option<u64>, Option<u64>), RpcError> {
    let (debit, receive) = match amount {
        Some(Amount_::DebitAmount(value)) => (Some(value), None),
        Some(Amount_::ReceiveAmount(value)) => (None, Some(value)),
        None => (None, None),
    };
    if [debit, receive]
        .into_iter()
        .flatten()
        .any(|value| value == 0 || value > dassie_ledger::MAX_TOTAL)
    {
        return Err(RpcError::invalid("the amount is out of range"));
    }
    Ok((debit, receive))
}

/// Creates the incoming payment a Send pays, at `wallet`, expecting
/// `amount` or any, with the note as its description; its ID, when it is
/// this node's.
async fn create_receiver(
    node: &Node,
    engine: &Engine,
    urls: &Urls,
    wallet: &Wallet,
    amount: Option<u64>,
    note: &str,
) -> Result<(String, Receiver, Option<PaymentId>), RpcError> {
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
                wallet_address: urls.wallet_address(ledger, false),
                client: None,
                incoming_amount: amount,
                expires_at_ms: Some(expires_at_ms),
                metadata: metadata.map(|metadata| serde_json::Value::Object(metadata).to_string()),
            })
            .await?;
        let payment = spending::local_receiver(node, urls, &row.id.to_string()).await?;
        return Ok((
            urls.incoming_payment(row.id),
            spending::receiver_of(node, payment)?,
            Some(row.id),
        ));
    }
    let remote = wallet
        .remote
        .as_ref()
        .ok_or_else(|| RpcError::internal("a wallet address that is neither"))?;
    let payment = client::create_incoming(node, remote, amount, expires_at_ms, metadata)
        .await
        .map_err(|error| client_error(&error))?;
    let url = payment.id.clone();
    Ok((url, spending::receiver_of(node, payment)?, None))
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
