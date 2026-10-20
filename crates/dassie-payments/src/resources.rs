//! The JSON of the wallet address server and the resource server:
//! wallet addresses, incoming payments, pages and errors.
//!
//! Requests are read strictly: unknown fields are refused where the specs
//! say `additionalProperties: false`, and every string and object has a
//! bound. Responses are written exactly as the specs' schemas describe
//! them, since the reference client validates them.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::amount::Amount;
use crate::uri::{self, InvalidUri};

/// The largest `metadata` object accepted, in bytes of compact JSON.
pub const MAX_METADATA_BYTES: usize = 4 * 1024;
/// The most items a page may hold.
pub const MAX_PAGE_ITEMS: u8 = 100;
/// The items on a page when the client names no number.
pub const DEFAULT_PAGE_ITEMS: u8 = 20;

/// A wallet address (`wallet-address`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletAddress {
    /// The wallet address's URL, as the client requested it.
    pub id: String,
    /// A name to show for the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_name: Option<String>,
    /// The account's asset.
    pub asset_code: String,
    /// The account's asset scale.
    pub asset_scale: u8,
    /// The authorization server's grant endpoint.
    pub auth_server: String,
    /// The resource server's base URL, without a trailing slash.
    pub resource_server: String,
    /// What a Dassie node says of itself, for Dassie-aware clients; the
    /// schema allows more properties, so other clients ignore it. Read
    /// leniently: anything but what a Dassie node writes is absent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient"
    )]
    pub dassie: Option<DassieWallet>,
}

/// Reads `dassie` as a Dassie node writes it, and anything else as absent.
/// The schema lets other servers use the name for something of their own,
/// and a wallet address that is payable without the field must stay so.
fn lenient<'de, D>(deserializer: D) -> Result<Option<DassieWallet>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// The `dassie` object of a wallet address served by a Dassie node
/// (spec/areas/payments.md#the-dassie-field).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DassieWallet {
    /// The node's ID, whose key signs its responses.
    pub node_id: String,
}

/// The ILP payment method: STREAM credentials (`ilp-payment-method`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IlpMethod {
    /// Always `ilp`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The STREAM destination.
    pub ilp_address: String,
    /// The STREAM shared secret, unpadded base64url.
    pub shared_secret: String,
}

/// An incoming payment, with its methods when the reader may pay it
/// (`incoming-payment`, `incoming-payment-with-methods`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingPayment {
    /// The payment's URL.
    pub id: String,
    /// The wallet address it pays into.
    pub wallet_address: String,
    /// Whether it accepts no more money.
    pub completed: bool,
    /// The most it accepts, if limited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incoming_amount: Option<Amount>,
    /// What it has received.
    pub received_amount: Amount,
    /// When it stops accepting money.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// The creator's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// When it was created.
    pub created_at: String,
    /// How to pay it, for readers allowed to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub methods: Option<Vec<IlpMethod>>,
}

/// What anyone may read of an incoming payment (`public-incoming-payment`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicIncomingPayment {
    /// What it has received.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_amount: Option<Amount>,
    /// Where to get a grant to read more.
    pub auth_server: String,
}

/// `POST /incoming-payments` (`create-incoming-payment-request`).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateIncomingPayment {
    /// The wallet address to pay into.
    pub wallet_address: String,
    /// The most it should accept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incoming_amount: Option<Amount>,
    /// When it should stop accepting money.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// The creator's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// A quote (`quote`): what an outgoing payment to `receiver` debits and
/// delivers, until it expires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Quote {
    /// The quote's URL.
    pub id: String,
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The incoming payment it pays.
    pub receiver: String,
    /// What the receiver gets.
    pub receive_amount: Amount,
    /// What the wallet address is debited.
    pub debit_amount: Amount,
    /// Always `ilp`.
    pub method: String,
    /// When it can no longer be paid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// When it was made.
    pub created_at: String,
}

/// `POST /quotes`: by the receiver's own amount, or with the amount to
/// debit or to deliver, never both (`create-quote-request`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateQuote {
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The incoming payment to pay.
    pub receiver: String,
    /// Always `ilp`.
    pub method: String,
    /// What to debit, for a fixed-send quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debit_amount: Option<Amount>,
    /// What to deliver, for a fixed-receive quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_amount: Option<Amount>,
}

/// An outgoing payment (`outgoing-payment`, and with the grant's spent
/// amounts once created).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingPayment {
    /// The payment's URL.
    pub id: String,
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The quote it pays, if made from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
    /// Whether it failed.
    pub failed: bool,
    /// The incoming payment it pays.
    pub receiver: String,
    /// What the receiver is to get.
    pub receive_amount: Amount,
    /// What the wallet address is to be debited.
    pub debit_amount: Amount,
    /// What was debited so far.
    pub sent_amount: Amount,
    /// The creator's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    /// When it was made.
    pub created_at: String,
    /// What the grant has spent in its current interval, with this
    /// payment; only in the response that creates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_spent_debit_amount: Option<Amount>,
    /// As `grant_spent_debit_amount`, as the receivers count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_spent_receive_amount: Option<Amount>,
}

/// `POST /outgoing-payments`: from a quote, or from an incoming payment
/// with the amount to debit (`create-outgoing-payment-request`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOutgoingPayment {
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The quote to pay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
    /// The incoming payment to pay, without a quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incoming_payment: Option<String>,
    /// What to debit, without a quote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debit_amount: Option<Amount>,
    /// The creator's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// What the grant behind a token has spent in its current interval
/// (`GET /outgoing-payment-grant`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingPaymentGrant {
    /// What it debited, if anything.
    pub spent_debit_amount: Option<Amount>,
    /// What it delivered, if anything.
    pub spent_receive_amount: Option<Amount>,
}

/// Whether a `metadata` object is within [`MAX_METADATA_BYTES`].
#[must_use]
pub fn metadata_fits(metadata: &Map<String, Value>) -> bool {
    serde_json::to_vec(metadata).is_ok_and(|json| json.len() <= MAX_METADATA_BYTES)
}

/// A page of a list (`page-info`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageInfo {
    /// The cursor of the first item on the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_cursor: Option<String>,
    /// The cursor of the last item on the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_cursor: Option<String>,
    /// Whether items follow the page.
    pub has_next_page: bool,
    /// Whether items precede the page.
    pub has_previous_page: bool,
}

/// A list response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    /// Where the page is in the list.
    pub pagination: PageInfo,
    /// Its items.
    pub result: Vec<T>,
}

/// Which way a page goes from its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The items after the cursor (`first`).
    Forward,
    /// The items before the cursor (`last`).
    Backward,
}

/// A list request's query: `wallet-address`, `cursor`, `first`, `last`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListQuery {
    /// The wallet address whose resources to list.
    pub wallet_address: String,
    /// Where to start; the list's own end when absent.
    pub cursor: Option<String>,
    /// Which way to go.
    pub direction: Direction,
    /// How many items, 1 through [`MAX_PAGE_ITEMS`].
    pub count: u8,
}

impl ListQuery {
    /// Reads a query string.
    ///
    /// # Errors
    ///
    /// If `wallet-address` is missing, `first` and `last` are both given,
    /// a count is out of range, or the query is malformed.
    pub fn parse(query: &str) -> Result<Self, InvalidUri> {
        let mut wallet_address = None;
        let mut cursor = None;
        let mut first = None;
        let mut last = None;
        for (key, value) in uri::query_pairs(query)? {
            let slot = match key.as_str() {
                "wallet-address" => &mut wallet_address,
                "cursor" => &mut cursor,
                "first" => &mut first,
                "last" => &mut last,
                _ => continue,
            };
            if slot.replace(value).is_some() {
                return Err(InvalidUri);
            }
        }
        let count = |value: String| {
            value
                .parse::<u8>()
                .ok()
                .filter(|count| (1..=MAX_PAGE_ITEMS).contains(count))
                .ok_or(InvalidUri)
        };
        let (direction, count) = match (first, last) {
            (Some(_), Some(_)) => return Err(InvalidUri),
            (None, Some(last)) => (Direction::Backward, count(last)?),
            (Some(first), None) => (Direction::Forward, count(first)?),
            (None, None) => (Direction::Forward, DEFAULT_PAGE_ITEMS),
        };
        Ok(Self {
            wallet_address: wallet_address.ok_or(InvalidUri)?,
            cursor: cursor.filter(|cursor| !cursor.is_empty()),
            direction,
            count,
        })
    }
}

/// An error response, `{"error": {"code", "description"}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// The error.
    pub error: ErrorDetail,
}

/// The inside of an [`ErrorBody`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// A machine-readable code, such as `invalid_request`.
    pub code: String,
    /// What went wrong, for people.
    pub description: String,
}

impl ErrorBody {
    /// An error with a code and description.
    #[must_use]
    pub fn new(code: &str, description: &str) -> Self {
        Self {
            error: ErrorDetail {
                code: code.to_string(),
                description: description.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_foreign_wallet_address_whatever_its_dassie_field() {
        let document = |dassie: &str| {
            alloc::format!(
                r#"{{"id":"https://a.example/pay","assetCode":"USD","assetScale":2,"authServer":"https://auth.a.example","resourceServer":"https://a.example","dassie":{dassie}}}"#
            )
        };
        for malformed in [
            r#""a string""#,
            "{}",
            r#"{"nodeId":7}"#,
            r#"{"nodeId":null}"#,
            "null",
            "[1,2]",
            "true",
        ] {
            let address: WalletAddress = serde_json::from_str(&document(malformed)).unwrap();
            assert_eq!(address.dassie, None, "{malformed}");
            assert_eq!(address.asset_code, "USD");
        }
        let address: WalletAddress =
            serde_json::from_str(&document(r#"{"nodeId":"dabc","relays":[]}"#)).unwrap();
        assert_eq!(
            address.dassie,
            Some(DassieWallet {
                node_id: "dabc".into()
            })
        );
        let without = r#"{"id":"https://a.example/pay","assetCode":"USD","assetScale":2,"authServer":"https://auth.a.example","resourceServer":"https://a.example"}"#;
        let address: WalletAddress = serde_json::from_str(without).unwrap();
        assert_eq!(address.dassie, None);
    }

    #[test]
    fn reads_a_create_request() {
        let json = r#"{"walletAddress":"https://a.example/pay","incomingAmount":{"value":"250","assetCode":"USD","assetScale":2},"expiresAt":"2022-04-12T23:20:50.52Z","metadata":{"description":"coffee"}}"#;
        let request: CreateIncomingPayment = serde_json::from_str(json).unwrap();
        assert_eq!(request.wallet_address, "https://a.example/pay");
        assert_eq!(request.incoming_amount, Some(Amount::new(250, "USD", 2)));
        assert!(metadata_fits(request.metadata.as_ref().unwrap()));
        let unknown = r#"{"walletAddress":"https://a.example/pay","receivedAmount":{"value":"1","assetCode":"USD","assetScale":2}}"#;
        assert!(serde_json::from_str::<CreateIncomingPayment>(unknown).is_err());
    }

    #[test]
    fn writes_an_incoming_payment_as_the_spec_does() {
        let payment = IncomingPayment {
            id: "https://b.example/incoming-payments/1".into(),
            wallet_address: "https://b.example/.well-known/pay".into(),
            completed: false,
            incoming_amount: None,
            received_amount: Amount::new(0, "USD", 9),
            expires_at: None,
            metadata: None,
            created_at: "2022-03-12T23:20:50.520Z".into(),
            methods: Some(alloc::vec![IlpMethod {
                kind: "ilp".into(),
                ilp_address: "test.b.stream".into(),
                shared_secret: "AAAA".into(),
            }]),
        };
        assert_eq!(
            serde_json::to_string(&payment).unwrap(),
            r#"{"id":"https://b.example/incoming-payments/1","walletAddress":"https://b.example/.well-known/pay","completed":false,"receivedAmount":{"value":"0","assetCode":"USD","assetScale":9},"createdAt":"2022-03-12T23:20:50.520Z","methods":[{"type":"ilp","ilpAddress":"test.b.stream","sharedSecret":"AAAA"}]}"#
        );
    }

    #[test]
    fn reads_list_queries() {
        let query =
            ListQuery::parse("wallet-address=https%3A%2F%2Fb.example%2Fpay&last=5&cursor=abc")
                .unwrap();
        assert_eq!(query.wallet_address, "https://b.example/pay");
        assert_eq!(query.direction, Direction::Backward);
        assert_eq!(query.count, 5);
        assert_eq!(query.cursor.as_deref(), Some("abc"));
        assert_eq!(
            ListQuery::parse("wallet-address=x").unwrap().count,
            DEFAULT_PAGE_ITEMS
        );
        for bad in [
            "first=5",
            "wallet-address=x&first=0",
            "wallet-address=x&first=101",
            "wallet-address=x&first=1&last=1",
            "wallet-address=x&wallet-address=y",
        ] {
            assert_eq!(ListQuery::parse(bad), Err(InvalidUri), "{bad}");
        }
    }
}
