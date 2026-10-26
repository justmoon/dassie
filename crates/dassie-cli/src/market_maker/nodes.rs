//! The market maker's two nodes, over their IPC sockets: what each holds,
//! which targets each is peered with, and a trade from one through a
//! target to the other (spec/areas/rate-feed.md#market-maker).
//!
//! A trade asks the receiving node for an incoming payment beneath the
//! target, so that the payment reaches it through that target and no
//! other, quotes paying
//! its STREAM credentials from the sending node with a fixed debit, and
//! pays only if the quote still clears the margin: the feed decides only
//! whether to quote, and the quote, whose rate every packet must then
//! deliver (spec/areas/payments.md#paying-stream-credentials), whether to
//! pay. Every call is bounded in time, and a connection that fails is made
//! again at the next call.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dassie_identity::NodeId;
use dassie_node::rpc::proto::{self, request::Method, response, stream_item};

use super::strategy::{Holding, Params, Pool, Side, Trade, check_quote};
use crate::client::Client;

/// How long one call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a payment is followed before the market maker moves on; the
/// node still finishes it.
const PAYMENT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the incoming payment of a trade takes money.
const INCOMING_TTL_MS: u64 = 10 * 60 * 1_000;
/// The scale both ledgers' books count at.
pub const LEDGER_SCALE: u32 = 9;

/// One of the nodes.
pub struct Node {
    /// Which.
    pub side: Side,
    /// The ledger it runs.
    pub ledger: String,
    socket: PathBuf,
    client: Option<Client>,
}

/// What `ListLedgers` says of a node's ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerState {
    /// Its asset's code.
    pub asset_code: String,
    /// What the node holds.
    pub holding: Holding,
    /// Its XRPL account's address.
    pub deposit_address: String,
    /// Who issues what it settles in, for a token; empty for XRP.
    pub issuer: String,
}

/// How a trade ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeOutcome {
    /// The quote no longer cleared the margin: nothing was paid.
    Declined {
        /// What the quote would have delivered.
        receive: u64,
    },
    /// The payment ended, or was left to the node.
    Paid {
        /// Its state: `completed`, `failed`, or `sending` if it was still
        /// going when the market maker stopped following it.
        state: String,
        /// What it sent.
        sent: u64,
        /// What the quote said it delivers.
        receive: u64,
    },
}

impl Node {
    /// The node of `side` at `socket`, running `ledger`; it connects at
    /// the first call.
    #[must_use]
    pub const fn new(side: Side, socket: PathBuf, ledger: String) -> Self {
        Self {
            side,
            ledger,
            socket,
            client: None,
        }
    }

    /// The socket.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Calls `method`, within [`CALL_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// The node cannot be reached, does not answer in time, or refuses.
    pub async fn call(&mut self, method: Method) -> Result<response::Method, String> {
        let outcome = tokio::time::timeout(CALL_TIMEOUT, async {
            let client = self.connected().await?;
            client.call(method).await.map_err(|e| (e.code, e.message))
        })
        .await
        .unwrap_or_else(|_| Err((None, "the node did not answer in time".into())));
        self.settle(outcome)
    }

    async fn connected(&mut self) -> Result<&mut Client, (Option<proto::ErrorCode>, String)> {
        if self.client.is_none() {
            let client = Client::connect_to(&self.socket)
                .await
                .map_err(|e| (None, e.message))?;
            self.client = Some(client);
        }
        self.client
            .as_mut()
            .ok_or_else(|| (None, "not connected".into()))
    }

    /// The outcome of a call: a connection that failed, rather than a node
    /// that refused, is dropped.
    fn settle<T>(
        &mut self,
        outcome: Result<T, (Option<proto::ErrorCode>, String)>,
    ) -> Result<T, String> {
        outcome.map_err(|(code, message)| {
            if code.is_none() {
                self.client = None;
            }
            format!("{} node: {message}", self.side.name())
        })
    }

    /// What the node holds on its ledger.
    ///
    /// # Errors
    ///
    /// The call failed, or the node does not run the ledger.
    pub async fn ledger_state(&mut self) -> Result<LedgerState, String> {
        let response::Method::ListLedgers(list) = self
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await?
        else {
            return Err("ListLedgers answered out of turn".into());
        };
        let ledger = list
            .ledgers
            .into_iter()
            .find(|ledger| ledger.id == self.ledger)
            .ok_or_else(|| format!("the {} node runs no {}", self.side.name(), self.ledger))?;
        if ledger.asset_scale != LEDGER_SCALE {
            return Err(format!(
                "{} counts at scale {}",
                ledger.id, ledger.asset_scale
            ));
        }
        Ok(LedgerState {
            asset_code: ledger.asset_code,
            holding: Holding {
                books: u64::try_from(ledger.owner_balance.max(0)).unwrap_or(0),
                on_ledger: ledger.on_ledger_balance,
            },
            deposit_address: ledger.deposit_address,
            issuer: ledger.issuer,
        })
    }

    /// The uplinks the node is confirmed with now: peerings it lists as
    /// its neighbors, which is when it takes an incoming payment beneath
    /// them (spec/areas/payments.md#receiving). The node's own list, so as
    /// many as it may peer with.
    ///
    /// # Errors
    ///
    /// The call failed.
    pub async fn confirmed_uplinks(&mut self) -> Result<BTreeSet<NodeId>, String> {
        let response::Method::ListPeers(list) = self
            .call(Method::ListPeers(proto::ListPeersRequest {}))
            .await?
        else {
            return Err("ListPeers answered out of turn".into());
        };
        Ok(list
            .peers
            .iter()
            .filter(|peer| peer.advertised && peer.state == "peered")
            .filter_map(|peer| NodeId::parse(&peer.node).ok())
            .collect())
    }
}

/// Carries out `trade` from `from` through `target` to `to`, checking its
/// quote against `pool` and `params` before paying, and follows the
/// payment to its end or [`PAYMENT_TIMEOUT`]. `to`'s credentials name the
/// address beneath `target`, and `from` is peered with `target`, so the
/// payment's first hop is `target`, and `target` converts it.
///
/// # Errors
///
/// A call failed before anything was paid, or while following; `to`
/// refuses a `target` that is not a confirmed uplink of its now.
pub async fn trade(
    from: &mut Node,
    to: &mut Node,
    target: NodeId,
    trade: &Trade,
    pool: &Pool,
    params: &Params,
    now_ms: u64,
) -> Result<TradeOutcome, String> {
    assert!(from.side == trade.from && to.side == trade.from.other());
    let incoming = create_incoming(to, target, now_ms).await?;
    let outcome = quote_and_pay(from, &incoming, trade, pool, params).await;
    // The incoming payment takes no more, whatever happened: its
    // credentials were for this trade only.
    let complete = Method::CompleteIncomingPayment(proto::CompleteIncomingPaymentRequest {
        id: incoming.id.clone(),
    });
    if let Err(problem) = to.call(complete).await {
        tracing::warn!(target: super::TARGET, %problem, "could not complete a trade's incoming payment");
    }
    outcome
}

/// An incoming payment on `to`, paid beneath `uplink`.
async fn create_incoming(
    to: &mut Node,
    uplink: NodeId,
    now_ms: u64,
) -> Result<proto::IncomingPayment, String> {
    let request = proto::CreateIncomingPaymentRequest {
        ledger: to.ledger.clone(),
        incoming_amount: None,
        expires_at_unix_ms: i64::try_from(now_ms.saturating_add(INCOMING_TTL_MS))
            .unwrap_or(i64::MAX),
        description: "market maker".into(),
        uplink: Some(uplink.to_string()),
    };
    match to.call(Method::CreateIncomingPayment(request)).await? {
        response::Method::CreateIncomingPayment(proto::CreateIncomingPaymentResponse {
            payment: Some(payment),
        }) => Ok(payment),
        _ => Err("CreateIncomingPayment answered out of turn".into()),
    }
}

async fn quote_and_pay(
    from: &mut Node,
    incoming: &proto::IncomingPayment,
    trade: &Trade,
    pool: &Pool,
    params: &Params,
) -> Result<TradeOutcome, String> {
    let request = proto::QuoteSendRequest {
        recipient: String::new(),
        credentials: Some(proto::StreamCredentials {
            destination_account: incoming.destination_account.clone(),
            shared_secret: incoming.shared_secret.clone(),
            asset_code: incoming.asset_code.clone(),
            asset_scale: incoming.asset_scale,
        }),
        ledger: from.ledger.clone(),
        amount: Some(proto::quote_send_request::Amount::DebitAmount(trade.debit)),
        note: String::new(),
    };
    let response::Method::QuoteSend(proto::QuoteSendResponse { quote: Some(quote) }) =
        from.call(Method::QuoteSend(request)).await?
    else {
        return Err("QuoteSend answered out of turn".into());
    };
    let receive = quote
        .receive_amount
        .as_ref()
        .filter(|amount| {
            amount.asset_code == incoming.asset_code && amount.asset_scale == LEDGER_SCALE
        })
        .map(|amount| amount.value)
        .ok_or("a quote in another asset")?;
    if quote.debit_amount > trade.debit
        || check_quote(trade.from, quote.debit_amount, receive, pool, params).is_err()
    {
        return Ok(TradeOutcome::Declined { receive });
    }
    let response::Method::SendPayment(proto::SendPaymentResponse {
        payment: Some(payment),
    }) = from
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: quote.id,
        }))
        .await?
    else {
        return Err("SendPayment answered out of turn".into());
    };
    let (state, sent) = follow(from, &payment.id).await?;
    Ok(TradeOutcome::Paid {
        state,
        sent,
        receive,
    })
}

/// Follows the outgoing payment `id` until it completes or fails, or
/// [`PAYMENT_TIMEOUT`] passes: its state and what it sent.
async fn follow(from: &mut Node, id: &str) -> Result<(String, u64), String> {
    let mut last = ("sending".to_owned(), 0);
    let method = Method::SubscribeOutgoingPayment(proto::SubscribeOutgoingPaymentRequest {
        id: id.to_owned(),
    });
    let outcome = tokio::time::timeout(PAYMENT_TIMEOUT, async {
        let client = from.connected().await?;
        client
            .stream_until(method, |item| match item {
                stream_item::Method::SubscribeOutgoingPayment(payment) => {
                    last = (payment.state.clone(), payment.sent_amount);
                    (payment.state != "sending").then_some(())
                }
                _ => None,
            })
            .await
            .map_err(|e| (e.code, e.message))
    })
    .await;
    match outcome {
        // Still sending when the wait ran out: the node finishes it, and
        // the connection, with the stream still open on it, is dropped.
        Err(_) => {
            from.client = None;
            Ok(last)
        }
        Ok(result) => from.settle(result).map(|_| last),
    }
}
