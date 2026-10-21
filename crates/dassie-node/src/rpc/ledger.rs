//! The methods of the ledger's engine: ledgers, BTP tokens, payments and
//! the debug views.

use std::sync::Arc;

use dassie_connector::{EndpointId, PacketRecord, PacketState, TraceEvent};
use dassie_ledger::{Reference, TraceId, TransferState};
use dassie_settlement::LedgerStatus;
use tokio::sync::broadcast::error::RecvError;

use super::handlers::{Items, Reply};
use super::live::{self, Follow, KEEPALIVE, Source};
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use crate::engine::{Engine, EngineError, LedgerView, RemovalBlocker, TokenView};
use crate::node::Node;
use crate::open_payments::auth::OpError;
use crate::store::JournalRow;

/// Page sizes for the list methods.
const DEFAULT_PAGE: u32 = 100;
const MAX_PAGE: u32 = 1_000;

/// Answers the engine's methods.
pub async fn dispatch(node: &Arc<Node>, method: Method, items: &Items) -> Result<Reply, RpcError> {
    match method {
        Method::ListTransfers(request) => return list_transfers(node, &request),
        Method::ListSettlementSchemes(_) => return list_schemes(node),
        Method::GetRateSources(_) | Method::SetRateSources(_) | Method::SetDisplayCurrency(_) => {
            return super::rates::dispatch(node, method).await;
        }
        _ => {}
    }
    let engine = node
        .engine()
        .ok_or_else(|| RpcError::unavailable(EngineError::Unavailable))?;
    let reply = match method {
        Method::ListLedgers(_) => {
            response::Method::ListLedgers(super::rates::ledgers(node, &engine).await?)
        }
        Method::AddSettlementScheme(request) => {
            let view = engine.add_scheme(request.id, request.config).await?;
            // Its currency may need a rate.
            node.wake_rates();
            response::Method::AddSettlementScheme(proto::AddSettlementSchemeResponse {
                ledger: Some(ledger(&view)),
            })
        }
        Method::RemoveSettlementScheme(request) => {
            engine.remove_scheme(request.id).await?;
            response::Method::RemoveSettlementScheme(proto::RemoveSettlementSchemeResponse {})
        }
        Method::StubDeposit(request) => {
            if request.amount == 0 {
                return Err(RpcError::invalid("the amount is zero"));
            }
            let view = engine.stub_deposit(request.amount).await?;
            response::Method::StubDeposit(proto::StubDepositResponse {
                ledger: Some(ledger(&view)),
            })
        }
        Method::AskFaucet(request) => {
            let view = engine.ask_faucet(request.settlement_scheme).await?;
            response::Method::AskFaucet(proto::AskFaucetResponse {
                ledger: Some(ledger(&view)),
            })
        }
        other => return dispatch_more(node, &engine, other, items).await,
    };
    Ok(Reply::Unary(reply))
}

async fn dispatch_more(
    node: &Arc<Node>,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::SubscribeLedgers(_) => return subscribe_ledgers(node, engine, items).await,
        Method::SubscribeTransfers(request) => {
            return subscribe_transfers(node, engine, &request, items).await;
        }
        Method::SetBtpTokenCap(request) => {
            let cap = request.cap.as_ref().map(cap);
            let view = engine.set_token_cap(request.id, cap).await?;
            response::Method::SetBtpTokenCap(proto::SetBtpTokenCapResponse {
                token: Some(token(&view)),
            })
        }
        Method::CreateBtpToken(request) => create_token(node, engine, request).await?,
        Method::ListBtpTokens(_) => response::Method::ListBtpTokens(proto::ListBtpTokensResponse {
            tokens: engine.tokens().await?.iter().map(token).collect(),
        }),
        Method::RevokeBtpToken(request) => {
            engine.revoke_token(request.id).await?;
            response::Method::RevokeBtpToken(proto::RevokeBtpTokenResponse {})
        }
        Method::ListAccounts(_) => response::Method::ListAccounts(proto::ListAccountsResponse {
            accounts: engine
                .accounts()
                .await?
                .iter()
                .map(|(id, account)| account_message(*id, account))
                .collect(),
        }),
        Method::ListPackets(request) => {
            let after = request.after.map(|trace| TraceId {
                epoch: trace.epoch,
                sequence: trace.sequence,
            });
            let limit = page(request.limit);
            let packets = engine.packets(after, limit).await?;
            response::Method::ListPackets(proto::ListPacketsResponse {
                packets: packets.iter().map(packet).collect(),
            })
        }
        Method::SubscribePackets(_) => {
            let _permit = permit(node)?;
            return subscribe_packets(engine, items).await;
        }
        other if super::grants::is_grants(&other) => {
            return super::grants::dispatch(node, engine, other, items).await;
        }
        other if super::send::is_send(&other) => {
            return super::send::dispatch(node, engine, other, items).await;
        }
        other if super::payments::is_payments(&other) => {
            return super::payments::dispatch(node, engine, other, items).await;
        }
        other => return super::network::dispatch(node, engine, other, items).await,
    };
    Ok(Reply::Unary(reply))
}

/// Streams the packet trace until the call ends or the engine stops.
async fn subscribe_packets(engine: &Engine, items: &Items) -> Result<Reply, RpcError> {
    let mut events = engine.subscribe_packets()?;
    let mut lost = 0u64;
    // An empty `lagged` says the stream is live, at once and then at least
    // every minute (spec/areas/rpc.md#live-streams).
    let alive = || {
        stream_item::Method::SubscribePackets(proto::PacketEvent {
            event: Some(proto::packet_event::Event::Lagged(0)),
        })
    };
    items.send_wait(alive()).await?;
    loop {
        let event = tokio::select! {
            () = engine.generation().cancelled() => {
                return Err(RpcError::unavailable(EngineError::Unavailable));
            }
            () = tokio::time::sleep(KEEPALIVE) => {
                items.try_send(alive());
                continue;
            }
            event = events.recv() => event,
        };
        let item = match event {
            Ok(TraceEvent::Packet(record)) => proto::packet_event::Event::Packet(packet(&record)),
            Ok(TraceEvent::Lagged(dropped)) | Err(RecvError::Lagged(dropped)) => {
                lost += dropped;
                continue;
            }
            Err(RecvError::Closed) => return Err(RpcError::unavailable(EngineError::Unavailable)),
        };
        // Whatever was lost, here or on the way, is reported before the
        // next packet, once there is room to say so.
        if lost > 0 {
            let lagged = proto::packet_event::Event::Lagged(lost);
            if !items.try_send(stream_item::Method::SubscribePackets(proto::PacketEvent {
                event: Some(lagged),
            })) {
                lost += 1;
                continue;
            }
            lost = 0;
        }
        if !items.try_send(stream_item::Method::SubscribePackets(proto::PacketEvent {
            event: Some(item),
        })) {
            lost += 1;
        }
    }
}

async fn create_token(
    node: &Node,
    engine: &Engine,
    request: proto::CreateBtpTokenRequest,
) -> Result<response::Method, RpcError> {
    let cap = request.cap.as_ref().map(cap);
    let created = engine
        .create_token(request.label, request.ledger, cap)
        .await?;
    let url = btp_url(node, &created.secret);
    Ok(response::Method::CreateBtpToken(
        proto::CreateBtpTokenResponse {
            token: Some(token(&created.view)),
            secret: created.secret,
            url,
        },
    ))
}

/// A place for one more live stream on the node.
pub(super) fn permit(node: &Node) -> Result<tokio::sync::OwnedSemaphorePermit, RpcError> {
    node.live_stream_permit()
        .ok_or_else(|| RpcError::exhausted("too many live streams"))
}

/// Where a stream on `engine` gets its wake-ups.
pub(super) fn source(engine: &Engine) -> Source {
    Source {
        versions: engine.versions(),
        ended: engine.generation().clone(),
    }
}

/// Streams the ledgers, whole, as they change.
async fn subscribe_ledgers(node: &Node, engine: &Engine, items: &Items) -> Result<Reply, RpcError> {
    let _permit = permit(node)?;
    // Removal blockers count peers, so the network matters too.
    live::values(
        source(engine),
        items,
        Follow::Both,
        || super::rates::ledgers(node, engine),
        stream_item::Method::SubscribeLedgers,
    )
    .await
}

/// Streams the journal, of one account if asked: its last rows, then each
/// row written or changed.
async fn subscribe_transfers(
    node: &Node,
    engine: &Engine,
    request: &proto::SubscribeTransfersRequest,
    items: &Items,
) -> Result<Reply, RpcError> {
    let _permit = permit(node)?;
    let account = request.account.map(dassie_ledger::AccountId);
    let history = u32::try_from(page(request.history)).unwrap_or(MAX_PAGE);
    let mut source = source(engine);
    let mut seen = source.seen(Follow::Ledger);
    let store = node.store();
    let rows = store
        .transfers_last(history, account)
        .map_err(RpcError::internal)?;
    let mut cursor = rows.last().map(|row| row.sequence);
    let mut shown = fetch_account(engine, account).await?;
    let event = proto::TransferEvent {
        reset: true,
        transfers: rows.iter().map(transfer).collect(),
        account: shown.clone(),
    };
    items
        .send_wait(stream_item::Method::SubscribeTransfers(event))
        .await?;
    let mut sent_at = tokio::time::Instant::now();
    loop {
        source.wait(Follow::Ledger, seen).await?;
        seen = source.seen(Follow::Ledger);
        loop {
            let rows = store
                .transfers_after(cursor, MAX_PAGE, account)
                .map_err(RpcError::internal)?;
            let now = fetch_account(engine, account).await?;
            let keepalive = sent_at.elapsed() >= KEEPALIVE;
            let full = rows.len() == usize::try_from(MAX_PAGE).unwrap_or(usize::MAX);
            if !rows.is_empty() || now != shown || keepalive {
                cursor = rows.last().map(|row| row.sequence).or(cursor);
                let event = proto::TransferEvent {
                    reset: false,
                    transfers: rows.iter().map(transfer).collect(),
                    account: now.clone(),
                };
                items
                    .send_wait(stream_item::Method::SubscribeTransfers(event))
                    .await?;
                sent_at = tokio::time::Instant::now();
                shown = now;
            }
            if !full {
                break;
            }
        }
    }
}

async fn fetch_account(
    engine: &Engine,
    account: Option<dassie_ledger::AccountId>,
) -> Result<Option<proto::Account>, RpcError> {
    let Some(id) = account else {
        return Ok(None);
    };
    Ok(engine
        .account(id)
        .await?
        .map(|account| account_message(id, &account)))
}

/// The schemes this node can run; answered without the engine.
fn list_schemes(node: &Node) -> Result<Reply, RpcError> {
    let active = node
        .store()
        .settlement_schemes()
        .map_err(RpcError::internal)?;
    let schemes = crate::engine::SCHEMES
        .iter()
        .map(|info| {
            let asset = info.ledger.asset();
            let realm = match info.ledger.realm() {
                dassie_ledger::Realm::Test => proto::Realm::Test,
                dassie_ledger::Realm::Live => proto::Realm::Live,
            };
            proto::SettlementSchemeInfo {
                id: info.id.into(),
                name: info.name.into(),
                ledger: info.ledger.as_str().into(),
                asset_code: asset.code.into(),
                asset_scale: asset.scale.into(),
                realm: realm.into(),
                active: active.iter().any(|(id, _)| id.as_str() == info.id),
            }
        })
        .collect();
    Ok(Reply::Unary(response::Method::ListSettlementSchemes(
        proto::ListSettlementSchemesResponse { schemes },
    )))
}

fn cap(cap: &proto::SpendingCap) -> dassie_connector::SpendingCap {
    dassie_connector::SpendingCap {
        amount: cap.amount,
        window_ms: cap.window_ms,
    }
}

fn list_transfers(node: &Node, request: &proto::ListTransfersRequest) -> Result<Reply, RpcError> {
    let rows = node
        .store()
        .transfers_after(
            request.after_sequence,
            u32::try_from(page(request.limit)).unwrap_or(MAX_PAGE),
            request.account.map(dassie_ledger::AccountId),
        )
        .map_err(RpcError::internal)?;
    Ok(Reply::Unary(response::Method::ListTransfers(
        proto::ListTransfersResponse {
            transfers: rows.iter().map(transfer).collect(),
        },
    )))
}

fn page(limit: u32) -> usize {
    let limit = match limit {
        0 => DEFAULT_PAGE,
        limit => limit.min(MAX_PAGE),
    };
    usize::try_from(limit).unwrap_or(100)
}

/// The URL a BTP client connects with: the node's hostname, or `localhost`
/// before one is set, and the HTTPS port if it is not 443.
fn btp_url(node: &Node, secret: &str) -> String {
    let config = node.config();
    let host = config
        .hostname
        .as_ref()
        .map_or("localhost", |name| name.as_str());
    match node.ports() {
        (_, 443) => format!("btp+wss://:{secret}@{host}/btp"),
        (_, port) => format!("btp+wss://:{secret}@{host}:{port}/btp"),
    }
}

impl From<OpError> for RpcError {
    /// An Open Payments error, by its status.
    fn from(error: OpError) -> Self {
        match error.status {
            400 => Self::invalid(error.description),
            401 | 403 | 409 => Self::precondition(error.description),
            404 => Self::not_found(error.description),
            429 => Self::exhausted(error.description),
            502 | 503 => Self::unavailable(error.description),
            // Logged where it was made.
            _ => Self::new(proto::ErrorCode::Internal, error.description),
        }
    }
}

impl From<EngineError> for RpcError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Unavailable => Self::unavailable(error),
            EngineError::Invalid(message) => Self::invalid(message),
            EngineError::Precondition(message) | EngineError::Refused(message) => {
                Self::precondition(message)
            }
            EngineError::NotFound(message) => Self::not_found(message),
            EngineError::Exhausted(message) => Self::exhausted(message),
            EngineError::Internal(message) => Self::internal(message),
        }
    }
}

fn saturate(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

pub(super) fn ledger(view: &LedgerView) -> proto::Ledger {
    let asset = view.id.asset();
    proto::Ledger {
        id: view.id.as_str().into(),
        settlement_scheme: view.scheme.clone(),
        asset_code: asset.code.into(),
        asset_scale: asset.scale.into(),
        owner_balance: saturate(view.owner_balance),
        owner_pending: u64::try_from(view.owner_pending).unwrap_or(u64::MAX),
        on_ledger_balance: view.on_ledger_balance,
        removal_blockers: view.removal_blockers.iter().map(blocker).collect(),
        reserve: view.reserve,
        deposit_address: view.deposit_address.clone().unwrap_or_default(),
        faucet: view.faucet,
        connection: connection(view.status).into(),
        connection_problem: match view.status {
            LedgerStatus::Failed(why) => why.into(),
            _ => String::new(),
        },
    }
}

const fn connection(status: LedgerStatus) -> proto::LedgerConnection {
    match status {
        LedgerStatus::Synced => proto::LedgerConnection::Synced,
        LedgerStatus::Connecting => proto::LedgerConnection::Connecting,
        LedgerStatus::Syncing => proto::LedgerConnection::Syncing,
        LedgerStatus::Unfunded => proto::LedgerConnection::Unfunded,
        LedgerStatus::Failed(_) => proto::LedgerConnection::Failed,
    }
}

fn blocker(blocker: &RemovalBlocker) -> proto::LedgerRemovalBlocker {
    use proto::ledger_removal_blocker::Kind;
    let (kind, count) = match *blocker {
        RemovalBlocker::Peers(count) => (Kind::Peers, count),
        RemovalBlocker::BtpTokens(count) => (Kind::BtpTokens, count),
        RemovalBlocker::OutgoingPayments(count) => (Kind::OutgoingPayments, count),
    };
    proto::LedgerRemovalBlocker {
        kind: kind.into(),
        count,
    }
}

fn token(view: &TokenView) -> proto::BtpToken {
    proto::BtpToken {
        id: view.id,
        label: view.label.clone(),
        ledger: view.ledger.as_str().into(),
        created_at_unix_ms: convert::time(view.created_at_ms),
        net_received: saturate(view.net_received),
        connections: u32::try_from(view.connections).unwrap_or(u32::MAX),
        cap: view.cap.map(|cap| proto::SpendingCap {
            amount: cap.amount,
            window_ms: cap.window_ms,
        }),
        cap_available: view.cap_available,
        cap_reserved: u64::try_from(view.cap_reserved).unwrap_or(u64::MAX),
        cap_full_at_unix_ms: view.cap_full_at_ms.map_or(0, convert::time),
    }
}

fn account_message(
    id: dassie_ledger::AccountId,
    account: &dassie_ledger::Account,
) -> proto::Account {
    let balance = account.balance();
    let counter = |value: u128| u64::try_from(value).unwrap_or(u64::MAX);
    proto::Account {
        id: id.0,
        path: account.path().to_string(),
        debits_posted: counter(balance.debits_posted),
        credits_posted: counter(balance.credits_posted),
        debits_pending: counter(balance.debits_pending),
        credits_pending: counter(balance.credits_pending),
    }
}

fn trace_id(trace: TraceId) -> proto::TraceId {
    proto::TraceId {
        epoch: trace.epoch,
        sequence: trace.sequence,
    }
}

fn transfer(row: &JournalRow) -> proto::Transfer {
    let reference = match row.transfer.reference {
        Reference::Packet(trace) => proto::transfer::Reference::Packet(trace_id(trace)),
        Reference::Settlement { scheme, id } => {
            proto::transfer::Reference::Settlement(format!("{scheme}:{id}"))
        }
        Reference::Trust => proto::transfer::Reference::Other("trust".into()),
        Reference::Deposit => proto::transfer::Reference::Other("deposit".into()),
        Reference::Withdrawal => proto::transfer::Reference::Other("withdrawal".into()),
        Reference::Reconciliation => proto::transfer::Reference::Other("reconciliation".into()),
        Reference::Refusal(trace) => proto::transfer::Reference::Refusal(trace_id(trace)),
        Reference::Fee { scheme, id } => proto::transfer::Reference::Fee(format!("{scheme}:{id}")),
    };
    let state = match row.state {
        TransferState::Pending => proto::TransferState::Pending,
        TransferState::Posted => proto::TransferState::Posted,
        TransferState::Voided => proto::TransferState::Voided,
    };
    proto::Transfer {
        id: row.transfer.id.0,
        debit: row.transfer.debit.0,
        credit: row.transfer.credit.0,
        debit_path: row.debit_path.clone(),
        credit_path: row.credit_path.clone(),
        amount: row.transfer.amount,
        state: state.into(),
        created_at_unix_ms: convert::time(row.transfer.created_at_ms),
        resolved_at_unix_ms: row.resolved_at_ms.map_or(0, convert::time),
        reference: Some(reference),
        sequence: row.sequence,
    }
}

fn endpoint(id: EndpointId) -> String {
    id.to_string()
}

fn packet(record: &PacketRecord) -> proto::Packet {
    let (state, code) = match record.state {
        PacketState::Pending => (proto::PacketState::Pending, String::new()),
        PacketState::Fulfilled => (proto::PacketState::Fulfilled, String::new()),
        PacketState::Rejected(code) => (proto::PacketState::Rejected, code.as_str().into()),
    };
    proto::Packet {
        trace: Some(trace_id(record.trace)),
        source: endpoint(record.source),
        destination: record.destination.map(endpoint).unwrap_or_default(),
        destination_address: record.destination_address().into(),
        amount_in: record.amount_in,
        amount_out: record.amount_out,
        received_at_unix_ms: convert::time(record.received_at_ms),
        resolved_at_unix_ms: record.resolved_at_ms.map_or(0, convert::time),
        state: state.into(),
        reject_code: code,
        transfers: record.transfers.iter().flatten().map(|id| id.0).collect(),
    }
}
