//! The market maker's two nodes, over their IPC sockets: what each holds,
//! which targets each is peered with, and a trade from one through a
//! target to the other (spec/areas/rate-feed.md#market-maker).
//!
//! Each node is a member of the settlement hub of its asset, and holds the
//! asset in two places: its account in the hub, on the hub's ledger, which
//! its trades pay from and are paid on, and the owner book of the asset on
//! its XRP Ledger account, which funds the hub and which its rebalances
//! spend from. The node moves funds between the two by itself, between the
//! hub's floor and ceiling (spec/areas/hubs.md#in-the-node), so what it
//! holds is both, and a move between them on its way.
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
    /// The ledger of its hub, which it trades on.
    pub ledger: String,
    /// The ledger of the owner book that funds the hub.
    pub owner_book: String,
    socket: PathBuf,
    client: Option<Client>,
}

/// What a node says of its asset: `ListLedgers` of its hub's ledger and
/// its owner book, and `GetHubStatus` of its hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerState {
    /// The asset's code, as both ledgers name it.
    pub asset_code: String,
    /// What the node holds.
    pub holding: Holding,
    /// The owner book's XRPL account's address.
    pub deposit_address: String,
    /// Who issues what the owner book holds, for a token; empty for XRP.
    pub issuer: String,
}

/// What the node's ledgers say, as [`holding`] reads them: the hub's
/// ledger and the owner book, each from `ListLedgers`, and from
/// `GetHubStatus` what is on its way into the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Views {
    /// The owner's funds on the hub's ledger.
    pub hub_owner: i64,
    /// The owner's funds in the owner book.
    pub book_owner: i64,
    /// Of those, what is held for payments on their way out of the book,
    /// such as a deposit into the hub.
    pub book_pending: u64,
    /// What the book's account holds, as the books have it, less what the
    /// ledger holds back.
    pub book_on_ledger: u64,
    /// A deposit into the hub that the XRP Ledger applied, so the book
    /// booked it out, and the hub has not credited yet: on neither ledger.
    pub arriving: u64,
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
    /// The node of `side` at `socket`, trading on its hub's ledger
    /// `ledger`, funded from the owner book `owner_book`; it connects at
    /// the first call.
    #[must_use]
    pub const fn new(side: Side, socket: PathBuf, ledger: String, owner_book: String) -> Self {
        Self {
            side,
            ledger,
            owner_book,
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

    /// What the node holds of its asset, in its hub and its owner book.
    ///
    /// # Errors
    ///
    /// A call failed, the node does not run either ledger, or they count
    /// another asset or at another scale.
    pub async fn ledger_state(&mut self) -> Result<LedgerState, String> {
        let response::Method::ListLedgers(list) = self
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await?
        else {
            return Err("ListLedgers answered out of turn".into());
        };
        let find = |id: &str| {
            list.ledgers
                .iter()
                .find(|ledger| ledger.id == id)
                .ok_or_else(|| format!("the {} node runs no {id}", self.side.name()))
        };
        let (hub, book) = (find(&self.ledger)?, find(&self.owner_book)?);
        for ledger in [hub, book] {
            if ledger.asset_scale != LEDGER_SCALE {
                return Err(format!(
                    "{} counts at scale {}",
                    ledger.id, ledger.asset_scale
                ));
            }
        }
        if hub.asset_code != book.asset_code {
            return Err(format!("{} and {} hold different assets", hub.id, book.id));
        }
        let scheme = hub.settlement_scheme.clone();
        let (asset_code, deposit_address, issuer) = (
            book.asset_code.clone(),
            book.deposit_address.clone(),
            book.issuer.clone(),
        );
        let views = Views {
            hub_owner: hub.owner_balance,
            book_owner: book.owner_balance,
            book_pending: book.owner_pending,
            book_on_ledger: book.on_ledger_balance,
            arriving: self.arriving(&scheme).await?,
        };
        Ok(LedgerState {
            asset_code,
            holding: holding(&views),
            deposit_address,
            issuer,
        })
    }

    /// A deposit into the hub over which `scheme` settles that the XRP
    /// Ledger applied and the hub has not credited yet: what is on neither
    /// of the node's ledgers (spec/areas/hubs.md#in-the-node).
    async fn arriving(&mut self, scheme: &str) -> Result<u64, String> {
        let response::Method::GetHubStatus(status) = self
            .call(Method::GetHubStatus(proto::GetHubStatusRequest {}))
            .await?
        else {
            return Err("GetHubStatus answered out of turn".into());
        };
        let hub = status
            .hubs
            .iter()
            .find(|hub| hub.scheme == scheme)
            .ok_or_else(|| format!("the {} node is in no hub over {scheme}", self.side.name()))?;
        Ok(hub.moving.as_ref().map_or(0, |moving| {
            let deposit = proto::hub_move::Direction::try_from(moving.direction)
                == Ok(proto::hub_move::Direction::Deposit);
            if deposit && moving.applied_at_unix_ms.is_some() {
                moving.amount
            } else {
                0
            }
        }))
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

/// What a node holds of its asset, from what its ledgers say: its funds
/// in the hub and in the owner book, none below zero, and a deposit on
/// neither, applied and not credited yet; of that, what the owner book's
/// account may spend now: what it holds, at most the owner's funds there
/// less what payments on their way out of it hold. A withdrawal is counted
/// on the hub's ledger until the hub executes it, and in the book once the
/// account sees it arrive: between, for as long as the escrow's payment
/// takes, the holding reads short of it, never above.
#[must_use]
pub fn holding(views: &Views) -> Holding {
    let positive = |balance: i64| u64::try_from(balance.max(0)).unwrap_or(0);
    let book = positive(views.book_owner);
    let total = positive(views.hub_owner)
        .saturating_add(book)
        .saturating_add(views.arriving);
    let on_ledger = views
        .book_on_ledger
        .min(book.saturating_sub(views.book_pending));
    assert!(
        on_ledger <= total,
        "the account's share is part of the whole"
    );
    Holding { total, on_ledger }
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

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i64 = 1_000_000_000;

    /// A node with `hub` in its hub and `book` in its owner book, all of
    /// the book in its account, and nothing on its way.
    const fn views(hub: i64, book: i64) -> Views {
        Views {
            hub_owner: hub,
            book_owner: book,
            book_pending: 0,
            book_on_ledger: book.unsigned_abs(),
            arriving: 0,
        }
    }

    const fn units(whole: i64) -> u64 {
        (whole * ONE).unsigned_abs()
    }

    #[test]
    fn a_node_holds_its_hub_and_its_book() {
        let held = holding(&views(5 * ONE, 95 * ONE));
        assert_eq!(held.total, units(100));
        // A rebalance spends only from the book's account.
        assert_eq!(held.on_ledger, units(95));
    }

    #[test]
    fn a_deposit_on_its_way_counts_once() {
        // Held in the book until the XRP Ledger applied it: still the
        // owner's there, but not the account's to spend.
        let mut leaving = views(5 * ONE, 95 * ONE);
        leaving.book_pending = units(10);
        assert_eq!(holding(&leaving).total, units(100));
        assert_eq!(holding(&leaving).on_ledger, units(85));
        // Applied: out of the book, not in the hub yet, so on its way.
        let mut arriving = views(5 * ONE, 85 * ONE);
        arriving.arriving = units(10);
        assert_eq!(holding(&arriving).total, units(100));
        assert_eq!(holding(&arriving).on_ledger, units(85));
    }

    #[test]
    fn the_account_spends_at_most_the_owners_funds() {
        // The account holds more than the books give the owner, or less.
        let mut more = views(0, 10 * ONE);
        more.book_on_ledger = units(12);
        assert_eq!(holding(&more).on_ledger, units(10));
        let mut less = views(0, 10 * ONE);
        less.book_on_ledger = units(4);
        assert_eq!(holding(&less).on_ledger, units(4));
    }

    #[test]
    fn nothing_counts_below_zero_or_overflows() {
        let held = holding(&views(-ONE, -2 * ONE));
        assert_eq!(held.total, 0);
        assert_eq!(held.on_ledger, 0);
        let mut most = views(i64::MAX, i64::MAX);
        most.arriving = u64::MAX;
        assert_eq!(holding(&most).total, u64::MAX);
    }
}
