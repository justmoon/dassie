//! The methods of the ledger's engine: ledgers, BTP tokens, payments and
//! the debug views.

use std::sync::Arc;

use dassie_connector::{EndpointId, PacketRecord, PacketState, TraceEvent};
use dassie_ledger::{Reference, TraceId, TransferState};
use tokio::sync::broadcast::error::RecvError;

use super::handlers::{Items, Reply};
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use crate::engine::{Engine, EngineError, LedgerView, TokenView};
use crate::node::Node;
use crate::store::JournalRow;

/// Page sizes for the list methods.
const DEFAULT_PAGE: u32 = 100;
const MAX_PAGE: u32 = 1_000;

/// Answers the engine's methods.
pub async fn dispatch(node: &Arc<Node>, method: Method, items: &Items) -> Result<Reply, RpcError> {
    if let Method::ListTransfers(request) = method {
        return list_transfers(node, &request);
    }
    let engine = node
        .engine()
        .ok_or_else(|| RpcError::unavailable(EngineError::Unavailable))?;
    let reply = match method {
        Method::ListLedgers(_) => response::Method::ListLedgers(proto::ListLedgersResponse {
            ledgers: engine.ledgers().await?.iter().map(ledger).collect(),
        }),
        Method::AddSettlementScheme(request) => {
            let view = engine.add_scheme(request.id, request.config).await?;
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
        Method::CreateBtpToken(request) => {
            let created = engine.create_token(request.label, request.ledger).await?;
            let url = btp_url(node, &created.secret);
            response::Method::CreateBtpToken(proto::CreateBtpTokenResponse {
                token: Some(token(&created.view)),
                secret: created.secret,
                url,
            })
        }
        Method::ListBtpTokens(_) => response::Method::ListBtpTokens(proto::ListBtpTokensResponse {
            tokens: engine.tokens().await?.iter().map(token).collect(),
        }),
        Method::RevokeBtpToken(request) => {
            engine.revoke_token(request.id).await?;
            response::Method::RevokeBtpToken(proto::RevokeBtpTokenResponse {})
        }
        Method::CreateStreamCredentials(request) => {
            let credentials = engine.stream_credentials(request.tag).await?;
            response::Method::CreateStreamCredentials(proto::CreateStreamCredentialsResponse {
                destination_account: credentials.destination,
                shared_secret: credentials.shared_secret.to_vec(),
                asset_code: credentials.asset.code.into(),
                asset_scale: credentials.asset.scale.into(),
            })
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
        Method::SubscribePackets(_) => return subscribe_packets(engine, items).await,
        other => return super::network::dispatch(engine, other).await,
    };
    Ok(Reply::Unary(reply))
}

/// Streams the packet trace until the call ends or the engine stops.
async fn subscribe_packets(engine: &Engine, items: &Items) -> Result<Reply, RpcError> {
    let mut events = engine.subscribe_packets()?;
    let mut lost = 0u64;
    loop {
        let event = tokio::select! {
            () = engine.generation().cancelled() => {
                return Err(RpcError::unavailable(EngineError::Unavailable));
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

fn list_transfers(node: &Node, request: &proto::ListTransfersRequest) -> Result<Reply, RpcError> {
    let rows = node
        .store()
        .transfers_after(
            request.after_sequence,
            u32::try_from(page(request.limit)).unwrap_or(MAX_PAGE),
            None,
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

impl From<EngineError> for RpcError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Unavailable => Self::unavailable(error),
            EngineError::Invalid(message) => Self::invalid(message),
            EngineError::Precondition(message) => Self::precondition(message),
            EngineError::NotFound(message) => Self::not_found(message),
            EngineError::Exhausted(message) => Self::exhausted(message),
            EngineError::Internal(message) => Self::internal(message),
        }
    }
}

fn saturate(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

fn ledger(view: &LedgerView) -> proto::Ledger {
    let asset = view.id.asset();
    proto::Ledger {
        id: view.id.as_str().into(),
        settlement_scheme: view.scheme.clone(),
        asset_code: asset.code.into(),
        asset_scale: asset.scale.into(),
        owner_balance: saturate(view.owner_balance),
        owner_pending: u64::try_from(view.owner_pending).unwrap_or(u64::MAX),
        on_ledger_balance: view.on_ledger_balance,
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
