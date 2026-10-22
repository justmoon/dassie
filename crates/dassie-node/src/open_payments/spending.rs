//! Spending, whoever asks (spec/decisions/open-payments.md): a third-party
//! client over HTTP, with a grant, or the owner over RPC, without one.
//!
//! A quote reads its receiver, here or on another node, measures the rate
//! of the path to it with a probe, and fixes what an outgoing payment
//! debits and delivers (M7c). It pays from the ledger the route to the
//! receiver starts on, since the node's own endpoints never convert. An
//! outgoing payment pays a quote of the same client, or an incoming
//! payment with the amount to debit, at the rate measured then.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dassie_ledger::LedgerId;
use dassie_payments::amount::Amount;
use dassie_payments::quote::Rate;
use dassie_payments::resources::{self, IncomingPayment};
use dassie_payments::time;
use dassie_stream::Credentials;

use super::auth::OpError;
use super::client::{self, ClientError};
use super::{INCOMING_PREFIX, Urls};
use crate::engine::{NewOutgoing, Prober};
use crate::node::Node;
use crate::store::{ForeignAmount, IncomingRow, PaymentId, QuoteRow};

/// How long a quote may be paid.
pub const QUOTE_LIFETIME_MS: u64 = 5 * 60 * 1_000;
/// The longest receiver URL kept.
pub const MAX_URL_BYTES: usize = 2_048;
/// The longest ILP address kept.
const MAX_ILP_ADDRESS_BYTES: usize = 1_023;
/// The longest asset code kept, as a payment's row holds it.
const MAX_ASSET_CODE_BYTES: usize = 32;

/// What paying an incoming payment needs to know of it.
#[derive(Clone, Debug)]
pub struct Receiver {
    /// Its STREAM destination.
    pub destination: String,
    /// Its STREAM shared secret.
    pub shared_secret: [u8; 32],
    /// The asset it is paid in.
    pub asset_code: String,
    /// The scale of that asset.
    pub asset_scale: u8,
    /// What it still accepts; none for no limit.
    pub remaining: Option<u64>,
    /// When it stops accepting money.
    pub expires_at_ms: Option<u64>,
}

/// Reads the incoming payment at `url`, here or on another node, with
/// the methods to pay it.
///
/// # Errors
///
/// If it is not an incoming payment's URL, or cannot be read.
pub async fn incoming_payment(
    node: &Node,
    urls: &Urls,
    url: &str,
) -> Result<IncomingPayment, OpError> {
    if url.len() > MAX_URL_BYTES || !dassie_payments::gnap::is_receiver(url) {
        return Err(OpError::invalid("the receiver is not an incoming payment"));
    }
    let local = urls.own_path(url).and_then(|path| {
        path.strip_prefix(&format!("{INCOMING_PREFIX}/"))
            .map(str::to_string)
    });
    match local {
        Some(id) => local_receiver(node, urls, &id).await,
        None => client::read_incoming(node, url)
            .await
            .map_err(|failure| match failure {
                ClientError::Unreachable(_) => OpError::new(
                    502,
                    "unreachable",
                    format!("the receiver cannot be read: {failure}"),
                ),
                ClientError::Unavailable => OpError::unavailable(),
                ClientError::Invalid(_) | ClientError::Refused(_) => {
                    OpError::invalid(format!("the receiver cannot be read: {failure}"))
                }
            }),
    }
}

/// Reads the incoming payment at `url`, and checks that it can be paid.
///
/// # Errors
///
/// As [`incoming_payment`] and [`receiver_of`].
pub async fn receiver(node: &Node, urls: &Urls, url: &str) -> Result<Receiver, OpError> {
    receiver_of(node, incoming_payment(node, urls, url).await?)
}

/// What paying `payment` needs: it must be open, unexpired, and payable
/// over ILP, with a valid address and secret.
///
/// # Errors
///
/// If it cannot be paid.
pub fn receiver_of(node: &Node, payment: IncomingPayment) -> Result<Receiver, OpError> {
    if payment.completed {
        return Err(OpError::invalid("the receiver accepts no more money"));
    }
    let expires_at_ms = payment
        .expires_at
        .as_deref()
        .map(time::parse_rfc3339)
        .transpose()
        .map_err(|_| OpError::invalid("the receiver's expiry is not a date-time"))?;
    if expires_at_ms.is_some_and(|expires| expires <= node.now_ms()) {
        return Err(OpError::invalid("the receiver has expired"));
    }
    let method = payment
        .methods
        .unwrap_or_default()
        .into_iter()
        .find(|method| method.kind == "ilp")
        .ok_or_else(|| OpError::invalid("the receiver cannot be paid over ILP"))?;
    // Another wallet says what it is: it must be an address.
    check_address(&method.ilp_address)?;
    let shared_secret = URL_SAFE_NO_PAD
        .decode(&method.shared_secret)
        .ok()
        .and_then(|secret| <[u8; 32]>::try_from(secret.as_slice()).ok())
        .ok_or_else(|| OpError::invalid("the receiver's shared secret is malformed"))?;
    let received = &payment.received_amount;
    let remaining = payment
        .incoming_amount
        .as_ref()
        .map(|incoming| incoming.value.saturating_sub(received.value));
    Ok(Receiver {
        destination: method.ilp_address,
        shared_secret,
        asset_code: received.asset_code.clone(),
        asset_scale: received.asset_scale,
        remaining,
        expires_at_ms,
    })
}

/// An ILP address a payment can be sent to, within the length a row keeps.
fn check_address(address: &str) -> Result<(), OpError> {
    if address.len() > MAX_ILP_ADDRESS_BYTES || dassie_ilp::Address::parse(address).is_err() {
        return Err(OpError::invalid("the receiver's ILP address is invalid"));
    }
    Ok(())
}

/// STREAM credentials as the owner names them, without an incoming
/// payment: an ILP address, and a shared secret of 32 bytes.
///
/// # Errors
///
/// If the address is no ILP address, or too long, or the secret is not
/// 32 bytes.
pub fn check_credentials(address: &str, shared_secret: &[u8]) -> Result<[u8; 32], OpError> {
    check_address(address)?;
    <[u8; 32]>::try_from(shared_secret)
        .map_err(|_| OpError::invalid("the shared secret is not 32 bytes"))
}

/// Whether `code` can be a payment's asset code: short, and printable
/// ASCII, so that a receiver's cannot read as something else to the owner.
#[must_use]
pub fn is_asset_code(code: &str) -> bool {
    (1..=MAX_ASSET_CODE_BYTES).contains(&code.len())
        && code.bytes().all(|byte| byte.is_ascii_graphic())
}

/// The receiver at STREAM credentials alone: no incoming payment says what
/// it accepts or until when, so the asset its amounts are in is the one
/// the owner `named`, as the incoming payment the credentials came with
/// says, or else the one the receiver `said` in the probe. Both must agree
/// when both are known, and one must be, since otherwise nothing would say
/// what the payment delivers. Either is refused unless its code is short
/// and printable, before the receiver's is ever shown to the owner.
///
/// # Errors
///
/// If no usable asset is known, or the two differ.
pub fn credentials_receiver(
    destination: String,
    shared_secret: [u8; 32],
    named: Option<(String, u8)>,
    said: Option<(String, u8)>,
) -> Result<Receiver, OpError> {
    let usable = |(code, _): &(String, u8)| is_asset_code(code);
    if !named.as_ref().is_none_or(usable) {
        return Err(OpError::invalid("the asset code is invalid"));
    }
    if !said.as_ref().is_none_or(usable) {
        return Err(OpError::invalid("the receiver's asset code is invalid"));
    }
    let (asset_code, asset_scale) = match (named, said) {
        (Some(named), Some(said)) if named != said => {
            return Err(OpError::invalid(format!(
                "the receiver says it is paid in {} at scale {}",
                said.0, said.1
            )));
        }
        (Some(asset), _) | (None, Some(asset)) => asset,
        (None, None) => {
            return Err(OpError::invalid(
                "the receiver does not say what asset it is paid in",
            ));
        }
    };
    Ok(Receiver {
        destination,
        shared_secret,
        asset_code,
        asset_scale,
        remaining: None,
        expires_at_ms: None,
    })
}

/// New STREAM credentials for an incoming payment of this node, while it
/// takes money: each read gets a connection of its own, so each payment
/// that pays it learns what it delivered, not what others did.
///
/// # Errors
///
/// If the engine is not running.
pub async fn fresh_credentials(
    node: &Node,
    row: &IncomingRow,
) -> Result<Option<Credentials>, OpError> {
    if !row.is_open(node.now_ms()) {
        return Ok(None);
    }
    let engine = node.engine().ok_or_else(OpError::unavailable)?;
    engine
        .incoming_credentials(row.id, row.ledger)
        .await
        .map(Some)
        .map_err(|_| OpError::unavailable())
}

/// An incoming payment of this node, as another node would read it.
///
/// # Errors
///
/// If there is none with that ID.
pub async fn local_receiver(
    node: &Node,
    urls: &Urls,
    id: &str,
) -> Result<IncomingPayment, OpError> {
    let not_found = || OpError::invalid("the receiver does not exist");
    let id = PaymentId::parse(id).ok_or_else(not_found)?;
    let row = node.store().incoming_payment(id)?.ok_or_else(not_found)?;
    let asset = row.ledger.asset();
    let amount = |value| Amount::new(value, asset.code, asset.scale);
    let open = row.is_open(node.now_ms());
    let credentials = fresh_credentials(node, &row).await?;
    Ok(IncomingPayment {
        id: urls.incoming_payment(id),
        wallet_address: row.wallet_address,
        completed: !open,
        incoming_amount: row.incoming_amount.map(amount),
        received_amount: amount(row.received_amount),
        expires_at: Some(time::format_rfc3339(row.expires_at_ms)),
        metadata: row
            .metadata
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok()),
        created_at: time::format_rfc3339(row.created_at_ms),
        methods: Some(credentials.into_iter().map(ilp_method).collect()),
    })
}

/// STREAM credentials as an Open Payments payment method.
#[must_use]
pub fn ilp_method(credentials: Credentials) -> resources::IlpMethod {
    resources::IlpMethod {
        kind: "ilp".into(),
        ilp_address: credentials.destination,
        shared_secret: URL_SAFE_NO_PAD.encode(credentials.shared_secret),
    }
}

/// The rate a payment from `ledger` to `receiver` is quoted at: what the
/// path delivers per unit sent, as a probe measures it, less STREAM's
/// slippage. One for one to the node's own receivers, which convert and
/// charge nothing. The probe takes a slot of `by`'s share
/// (spec/decisions/owners-share.md).
///
/// # Errors
///
/// If there is no route to the receiver, it starts on another ledger, or
/// the rate cannot be measured.
pub async fn quote_rate(
    node: &Node,
    ledger: LedgerId,
    receiver: &Receiver,
    by: Prober,
) -> Result<Rate, OpError> {
    let credentials = (receiver.destination.as_str(), receiver.shared_secret);
    let (rate, _) = measure(node, ledger, credentials, by).await?;
    Ok(rate)
}

/// The same for the receiver at STREAM credentials, with the asset it
/// says it is paid in, if it said one: the ledger's own for the node's own
/// receivers, which are on it.
///
/// # Errors
///
/// As [`quote_rate`].
pub async fn measure(
    node: &Node,
    ledger: LedgerId,
    (destination, shared_secret): (&str, [u8; 32]),
    by: Prober,
) -> Result<(Rate, Option<(String, u8)>), OpError> {
    let engine = node.engine().ok_or_else(OpError::unavailable)?;
    let route = engine
        .route_of(destination.to_string())
        .await
        .map_err(|_| OpError::unavailable())?;
    let Some((route_ledger, own)) = route else {
        return Err(OpError::new(
            502,
            "unreachable",
            "there is no route to the receiver",
        ));
    };
    if route_ledger != ledger {
        return Err(OpError::invalid(format!(
            "pay from {route_ledger}: the route to the receiver starts there"
        )));
    }
    if own {
        let asset = ledger.asset();
        return Ok((Rate::ONE, Some((asset.code.to_string(), asset.scale))));
    }
    let measured = engine
        .probe(ledger, destination.to_string(), shared_secret, by)
        .await
        .map_err(|error| match error {
            crate::engine::EngineError::Exhausted(message) => {
                OpError::new(429, "too_many_requests", message)
            }
            _ => OpError::unavailable(),
        })?
        .ok_or_else(|| {
            OpError::new(
                502,
                "unreachable",
                "the rate to the receiver could not be measured",
            )
        })?;
    let rate = measured
        .rate
        .with_slippage(dassie_stream::Policy::DEFAULT.slippage_permille);
    if rate.is_zero() {
        return Err(OpError::new(
            502,
            "unreachable",
            "the path to the receiver delivers nothing",
        ));
    }
    let rate = Rate {
        destination: rate.numerator(),
        source: rate.denominator(),
    };
    Ok((rate, measured.asset))
}

/// What a payment to `receiver` from `ledger` debits and delivers at
/// `rate`: from the amount to debit, the amount to deliver, or what the
/// receiver still accepts.
///
/// # Errors
///
/// If both or neither amount is named where one must be, an amount is in
/// the wrong asset, or the amounts are zero, out of range, or more than
/// it accepts.
pub fn amounts(
    ledger: LedgerId,
    receiver: &Receiver,
    rate: Rate,
    debit: Option<&Amount>,
    receive: Option<&Amount>,
) -> Result<(u64, u64), OpError> {
    let asset = ledger.asset();
    let (debit, receive) = match (debit, receive) {
        (Some(_), Some(_)) => {
            return Err(OpError::invalid(
                "name the amount to debit or to deliver, not both",
            ));
        }
        (Some(debit), None) if debit.same_asset(asset.code, asset.scale) => {
            (Some(debit.value), rate.delivered(debit.value))
        }
        (None, Some(receive)) if receive.same_asset(&receiver.asset_code, receiver.asset_scale) => {
            (rate.cost(receive.value), Some(receive.value))
        }
        (None, None) => {
            let remaining = receiver
                .remaining
                .ok_or_else(|| OpError::invalid("name an amount: the receiver takes any"))?;
            (rate.cost(remaining), Some(remaining))
        }
        _ => return Err(OpError::invalid("the amount is in another asset")),
    };
    let (Some(debit), Some(receive)) = (debit, receive) else {
        return Err(OpError::invalid("the amount is out of range"));
    };
    if debit > dassie_ledger::MAX_TOTAL || receive > dassie_ledger::MAX_TOTAL {
        return Err(OpError::invalid("the amount is out of range"));
    }
    if debit == 0 || receive == 0 {
        return Err(OpError::invalid("the amount is zero"));
    }
    if receiver
        .remaining
        .is_some_and(|remaining| receive > remaining)
    {
        return Err(OpError::invalid("the receiver does not accept that much"));
    }
    Ok((debit, receive))
}

/// A quote to make.
pub struct NewQuote {
    /// The ledger that pays.
    pub ledger: LedgerId,
    /// The client that asks; none for the owner.
    pub client: Option<String>,
    /// The incoming payment it pays.
    pub receiver_url: String,
    /// What paying it needs.
    pub receiver: Receiver,
    /// What it debits.
    pub debit: u64,
    /// What the receiver gets.
    pub receive: u64,
    /// The wallet address it pays, when the owner named one.
    pub recipient: Option<String>,
    /// Whether it fixes what is sent.
    pub fixed_debit: bool,
}

/// Stores a quote: payable for [`QUOTE_LIFETIME_MS`], or until its
/// receiver expires if that is sooner.
///
/// # Errors
///
/// 429 when too many quotes are live.
pub fn insert_quote(node: &Node, urls: &Urls, new: NewQuote) -> Result<QuoteRow, OpError> {
    let now = node.now_ms();
    let lifetime = now + QUOTE_LIFETIME_MS;
    let row = QuoteRow {
        id: PaymentId::from_random(super::auth::random_bytes(node)),
        ledger: new.ledger,
        wallet_address: urls.wallet_address(new.ledger, false),
        client: new.client,
        receiver: new.receiver_url,
        destination: new.receiver.destination,
        shared_secret: new.receiver.shared_secret,
        debit_amount: new.debit,
        receive_amount: ForeignAmount {
            value: new.receive,
            asset_code: new.receiver.asset_code,
            asset_scale: new.receiver.asset_scale,
        },
        created_at_ms: now,
        expires_at_ms: new
            .receiver
            .expires_at_ms
            .map_or(lifetime, |at| at.min(lifetime)),
        recipient: new.recipient,
        fixed_debit: new.fixed_debit,
    };
    if !node.store().insert_quote(&row, now)? {
        return Err(OpError::new(
            429,
            "too_many_requests",
            "too many quotes are live",
        ));
    }
    Ok(row)
}

/// An outgoing payment that pays a quote: one of `client`'s (none for the
/// owner's), on `ledger` if one is named, unexpired and not paid yet.
///
/// # Errors
///
/// If the quote does not exist, is another's, has expired, or was paid.
pub fn from_quote(
    node: &Node,
    id: PaymentId,
    client: Option<&str>,
    ledger: Option<LedgerId>,
) -> Result<NewOutgoing, OpError> {
    let quote = node
        .store()
        .quote(id)?
        .ok_or_else(|| OpError::invalid("the quote does not exist"))?;
    if ledger.is_some_and(|ledger| ledger != quote.ledger) || quote.client.as_deref() != client {
        return Err(OpError::forbidden("the quote is another's"));
    }
    if quote.expires_at_ms <= node.now_ms() {
        return Err(OpError::invalid("the quote has expired"));
    }
    if node.store().quote_used(id)? {
        return Err(OpError::invalid("the quote was paid already"));
    }
    Ok(NewOutgoing {
        ledger: quote.ledger,
        wallet_address: quote.wallet_address,
        client: None,
        grant: None,
        quote_id: Some(id),
        receiver: quote.receiver,
        destination: quote.destination,
        shared_secret: quote.shared_secret,
        debit_amount: quote.debit_amount,
        receive_amount: quote.receive_amount,
        metadata: None,
        recipient: quote.recipient,
        fixed_debit: quote.fixed_debit,
    })
}

/// An outgoing payment that pays an incoming payment, debiting `debit`,
/// at the rate measured now for `by`.
///
/// # Errors
///
/// As [`receiver`], [`quote_rate`] and [`amounts`].
pub async fn from_incoming(
    node: &Node,
    urls: &Urls,
    ledger: LedgerId,
    incoming: &str,
    debit: &Amount,
    by: Prober,
) -> Result<NewOutgoing, OpError> {
    let receiver = receiver(node, urls, incoming).await?;
    let rate = quote_rate(node, ledger, &receiver, by).await?;
    let (debit, receive) = amounts(ledger, &receiver, rate, Some(debit), None)?;
    Ok(NewOutgoing {
        ledger,
        wallet_address: urls.wallet_address(ledger, false),
        client: None,
        grant: None,
        quote_id: None,
        receiver: incoming.to_string(),
        destination: receiver.destination,
        shared_secret: receiver.shared_secret,
        debit_amount: debit,
        receive_amount: ForeignAmount {
            value: receive,
            asset_code: receiver.asset_code,
            asset_scale: receiver.asset_scale,
        },
        metadata: None,
        recipient: None,
        fixed_debit: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(code: &str, scale: u8) -> Option<(String, u8)> {
        Some((code.into(), scale))
    }

    fn receiver(
        named: Option<(String, u8)>,
        said: Option<(String, u8)>,
    ) -> Result<(String, u8), String> {
        credentials_receiver("test.a.b".into(), [7; 32], named, said)
            .map(|receiver| (receiver.asset_code, receiver.asset_scale))
            .map_err(|error| error.description)
    }

    // spec/areas/payments.md#paying-stream-credentials: credentials are an ILP
    // address a row can keep and a secret of 32 bytes.
    #[test]
    fn credentials_are_an_address_and_a_32_byte_secret() {
        assert_eq!(check_credentials("test.a.b", &[1; 32]), Ok([1; 32]));
        for secret in [&[1u8; 31][..], &[1; 33], &[]] {
            assert!(check_credentials("test.a.b", secret).is_err());
        }
        let long = format!("test.{}", "a".repeat(MAX_ILP_ADDRESS_BYTES));
        for address in ["", "test.", "not an address", "test..a", long.as_str()] {
            assert!(check_credentials(address, &[1; 32]).is_err(), "{address}");
        }
    }

    // spec/areas/payments.md#paying-stream-credentials: the asset is the one named,
    // else the one the receiver said; they must agree, and one must be
    // known.
    #[test]
    fn a_credentials_receiver_has_a_known_asset() {
        let usd = asset("USD", 9);
        assert_eq!(receiver(usd.clone(), None), Ok(("USD".into(), 9)));
        assert_eq!(receiver(None, usd.clone()), Ok(("USD".into(), 9)));
        assert_eq!(receiver(usd.clone(), usd.clone()), Ok(("USD".into(), 9)));
        assert!(receiver(usd.clone(), asset("USD", 6)).is_err());
        assert!(receiver(usd, asset("XRP", 9)).is_err());
        assert!(receiver(None, None).is_err());
        // What the receiver says is shown to the owner only once it is a
        // short printable code.
        let long = "A".repeat(MAX_ASSET_CODE_BYTES + 1);
        for code in ["", "U\u{202E}D", "a b", long.as_str()] {
            assert_eq!(
                receiver(None, asset(code, 2)),
                Err("the receiver's asset code is invalid".into()),
                "{code:?}"
            );
            assert!(receiver(asset(code, 2), None).is_err(), "{code:?}");
        }
        let receiver = credentials_receiver("test.a.b".into(), [7; 32], asset("XRP", 6), None);
        let receiver = receiver.unwrap();
        assert_eq!((receiver.remaining, receiver.expires_at_ms), (None, None));
    }
}
