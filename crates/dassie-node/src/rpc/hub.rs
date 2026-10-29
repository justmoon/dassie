//! The hub member's methods (`proto/dassie/owner/v1/hub_member.proto`):
//! the node's account in each settlement hub it is in, and the owner's
//! moves between it and the owner book of the hub's asset
//! (spec/areas/hubs.md#in-the-node).

use dassie_hub::ledger::Outcome;
use dassie_hub::member::Purpose;
use tokio::sync::broadcast::error::RecvError;

use super::handlers::{Items, Reply};
use super::live::KEEPALIVE;
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use crate::engine::{
    Direction, Engine, EngineError, HubAccountState, HubEvent, HubEventKind, HubHold, HubMoveEvent,
    HubOperation, HubPromise, HubStatus, HubStreamEvent, MoveStage, Role,
};

/// Whether `method` is one of the hub member's.
pub(super) const fn is_hub(method: &Method) -> bool {
    matches!(
        method,
        Method::GetHubStatus(_)
            | Method::HubDeposit(_)
            | Method::HubWithdraw(_)
            | Method::SetHubAutomatic(_)
            | Method::SetHubTarget(_)
            | Method::HubGiveUp(_)
            | Method::LeaveHub(_)
    )
}

/// What [`dispatch`] answers a unary method with, for the engine's tests
/// of the owner's calls; `None` for a streaming one.
#[cfg(test)]
pub(crate) async fn unary(
    engine: &Engine,
    method: Method,
) -> Result<Option<response::Method>, RpcError> {
    Ok(match dispatch(engine, method).await? {
        Reply::Unary(response) => Some(response),
        Reply::StreamEnd => None,
    })
}

/// Answers the hub member's methods.
pub(super) async fn dispatch(engine: &Engine, method: Method) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::GetHubStatus(_) => response::Method::GetHubStatus(proto::GetHubStatusResponse {
            hubs: engine
                .hub_statuses()
                .await?
                .iter()
                .map(hub_status)
                .collect(),
        }),
        Method::HubDeposit(request) => {
            let status = engine.hub_deposit(request.scheme, request.amount).await?;
            response::Method::HubDeposit(proto::HubDepositResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::HubWithdraw(request) => {
            let status = engine.hub_withdraw(request.scheme, request.amount).await?;
            response::Method::HubWithdraw(proto::HubWithdrawResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::SetHubAutomatic(request) => {
            let status = engine
                .set_hub_automatic(request.scheme, request.automatic)
                .await?;
            response::Method::SetHubAutomatic(proto::SetHubAutomaticResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::SetHubTarget(request) => {
            let status = engine
                .set_hub_target(request.scheme, request.target)
                .await?;
            response::Method::SetHubTarget(proto::SetHubTargetResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::HubGiveUp(request) => {
            let status = engine.hub_give_up(request.scheme).await?;
            response::Method::HubGiveUp(proto::HubGiveUpResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::LeaveHub(request) => {
            let gone = engine.leave_hub(request.scheme).await?;
            response::Method::LeaveHub(proto::LeaveHubResponse { gone })
        }
        _ => return Err(RpcError::internal("not a hub method")),
    };
    Ok(Reply::Unary(reply))
}

fn hub_status(status: &HubStatus) -> proto::HubStatus {
    let hex: String = status
        .hub_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let (validators, answering) = status.validators;
    // A move counts the hub asset's smallest unit, drops of XRP or
    // millionths of a token, which the books count alike: one rate takes
    // either to the ledger's units, as every other amount here is.
    let units = |smallest: u64| smallest.saturating_mul(dassie_hub::member::UNITS_PER_DROP);
    proto::HubStatus {
        hub_id: hex,
        scheme: status.scheme.as_str().into(),
        account: status.account,
        connection: super::ledger::connection(status.status).into(),
        balance: status.balance,
        reserve: status.reserve,
        deposited: status.deposited,
        withdrawn: status.withdrawn,
        returned: status.returned,
        paying: status.paying.iter().map(hold).collect(),
        taking: status.taking.iter().map(hold).collect(),
        target: status.target,
        target_by_owner: status.target_by_owner,
        floor: status.floor,
        ceiling: status.ceiling,
        automatic: status.automatic,
        paused_until_unix_ms: status.paused_until_ms.map(convert::time),
        moving: status.moving.map(|moving| proto::HubMove {
            direction: direction_of(moving.direction).into(),
            amount: units(moving.drops),
            by_owner: moving.owner,
            sent_at_unix_ms: convert::time(moving.sent_ms),
            applied_at_unix_ms: moving.applied_ms.map(convert::time),
            late: status.late,
        }),
        given_up: status.given_up.map(|(direction, drops)| proto::HubMove {
            direction: direction_of(direction).into(),
            amount: units(drops),
            ..Default::default()
        }),
        validators: u32::try_from(validators).unwrap_or(u32::MAX),
        validators_answering: u32::try_from(answering).unwrap_or(u32::MAX),
        // The hub's clock is the Unix time its validators stamp blocks with.
        frozen_since_unix_ms: status.frozen_since_ms.map(convert::time),
        escrow_held: status.escrow_held,
        settling: status.settling,
        liquidity: status.liquidity.map(|liquidity| proto::HubLiquidity {
            pool: liquidity.pool,
            peerings: u32::try_from(liquidity.peerings).unwrap_or(u32::MAX),
            covered: liquidity.covered,
            short_since_unix_ms: liquidity.short_since_ms.map(convert::time),
            ends_from_unix_ms: liquidity.ends_from_ms.map(convert::time),
            refusing_downlinks: liquidity.refusing_downlinks,
            refusing_core_peers: liquidity.refusing_core,
        }),
        stopped: status.stopped,
        ended: status.ended,
    }
}

const fn direction_of(direction: Direction) -> proto::hub_move::Direction {
    match direction {
        Direction::Deposit => proto::hub_move::Direction::Deposit,
        Direction::Withdraw => proto::hub_move::Direction::Withdraw,
    }
}

fn hold(hold: &HubHold) -> proto::HubHold {
    proto::HubHold {
        peer: hold.peer.map(|node| node.to_string()).unwrap_or_default(),
        account: hold.account,
        amount: hold.amount,
        allotment: hold.allotment,
    }
}

/// Streams what the node sees of its hubs until the call ends or the
/// engine stops, as `subscribe_packets` streams the packet trace.
pub(super) async fn subscribe_events(engine: &Engine, items: &Items) -> Result<Reply, RpcError> {
    let mut events = engine.subscribe_hub_events()?;
    let mut lost = 0u64;
    let lagged = |count: u64| {
        stream_item::Method::SubscribeHubEvents(proto::HubEvent {
            scheme: String::new(),
            observed_at_unix_ms: 0,
            event: Some(proto::hub_event::Event::Lagged(count)),
        })
    };
    // An empty `lagged` says the stream is live, at once and then at least
    // every minute (spec/areas/rpc.md#live-streams).
    items.send_wait(lagged(0)).await?;
    loop {
        let event = tokio::select! {
            () = engine.generation().cancelled() => {
                return Err(RpcError::unavailable(EngineError::Unavailable));
            }
            () = tokio::time::sleep(KEEPALIVE) => {
                items.try_send(lagged(0));
                continue;
            }
            event = events.recv() => event,
        };
        let item = match event {
            Ok(HubStreamEvent::Event(event)) => hub_event(&event),
            Ok(HubStreamEvent::Lagged(dropped)) | Err(RecvError::Lagged(dropped)) => {
                lost += dropped;
                continue;
            }
            Err(RecvError::Closed) => return Err(RpcError::unavailable(EngineError::Unavailable)),
        };
        // Whatever was lost, here or on the way, is told before the next
        // event, once there is room to say so.
        if lost > 0 {
            if !items.try_send(lagged(lost)) {
                lost += 1;
                continue;
            }
            lost = 0;
        }
        if !items.try_send(stream_item::Method::SubscribeHubEvents(item)) {
            lost += 1;
        }
    }
}

/// Why we submitted an operation, as the contract spells it.
fn purpose(purpose: Purpose) -> String {
    match purpose {
        Purpose::Settle(id) => format!("settle:{id}"),
        Purpose::Redeem(hold) => format!("redeem:{}", hold.0),
        Purpose::Claim(hold) => format!("claim:{}", hold.0),
        Purpose::Dispute(hold) => format!("dispute:{}", hold.0),
        Purpose::Disbursal => "disbursal".into(),
        Purpose::Withdraw => "withdraw".into(),
    }
}

fn role(role: Role) -> i32 {
    match role {
        Role::Payer => proto::HubRole::Payer,
        Role::Payee => proto::HubRole::Payee,
    }
    .into()
}

fn commitment(promise: &HubPromise) -> proto::HubCommitment {
    proto::HubCommitment {
        role: role(promise.role),
        peer: promise.peer.to_string(),
        hold: promise.hold,
        sequence: promise.sequence,
        amount: promise.amount,
        condition: promise.condition.to_vec(),
        expiry_unix_ms: convert::time(promise.expiry_ms),
    }
}

fn claim(promise: &HubPromise) -> proto::HubClaim {
    proto::HubClaim {
        role: role(promise.role),
        peer: promise.peer.to_string(),
        hold: promise.hold,
        total: promise.amount,
        through: promise.sequence,
    }
}

/// An outcome as the contract spells it: what became of it, why, and its
/// fee.
fn outcome(outcome: Outcome) -> (String, String, u64) {
    match outcome {
        Outcome::Applied { fee, .. } => ("applied".into(), String::new(), fee),
        Outcome::Failed { fee, reason } => ("failed".into(), format!("{reason:?}"), fee),
        Outcome::Refused(reason) => ("refused".into(), format!("{reason:?}"), 0),
    }
}

fn hub_operation(operation: &HubOperation) -> proto::HubOperation {
    let (outcome, reason, fee) = outcome(operation.outcome);
    proto::HubOperation {
        height: operation.height,
        timestamp_unix_ms: convert::time(operation.timestamp_ms),
        kind: operation.kind.into(),
        signer: operation.signer,
        sequence: operation.sequence,
        hold: operation.hold.unwrap_or(0),
        peer_account: operation.peer_account.unwrap_or(0),
        amount: operation.amount,
        outcome,
        reason,
        fee,
        purpose: operation.ours.map(purpose).unwrap_or_default(),
    }
}

const fn account_state(state: &HubAccountState) -> proto::HubAccountState {
    proto::HubAccountState {
        height: state.height,
        number: match state.number {
            Some(number) => number,
            None => 0,
        },
        balance: state.balance,
        reserve: state.reserve,
        sequence: state.sequence,
        deposited: state.deposited,
        withdrawn: state.withdrawn,
        returned: state.returned,
        closing: state.closing,
    }
}

fn move_event(moved: &HubMoveEvent) -> proto::HubMoveEvent {
    let stage = match moved.stage {
        MoveStage::Started => proto::hub_move_event::Stage::Started,
        MoveStage::Done => proto::hub_move_event::Stage::Done,
        MoveStage::Failed => proto::hub_move_event::Stage::Failed,
        MoveStage::Late => proto::hub_move_event::Stage::Late,
        MoveStage::GivenUp => proto::hub_move_event::Stage::GivenUp,
    };
    proto::HubMoveEvent {
        stage: stage.into(),
        direction: direction_of(moved.direction).into(),
        amount: moved.amount,
        payment: moved.payment.to_string(),
        by_owner: moved.owner,
    }
}

/// What the node saw of a hub, as the contract has it: `SubscribeHubEvents`
/// streams it, and the event log writes it (`crate::event_log`).
pub(crate) fn hub_event(event: &HubEvent) -> proto::HubEvent {
    use proto::hub_event::Event;
    let kind = match &event.kind {
        HubEventKind::Block {
            height,
            timestamp_ms,
        } => Event::Block(proto::HubBlock {
            height: *height,
            timestamp_unix_ms: convert::time(*timestamp_ms),
        }),
        HubEventKind::Operation(operation) => Event::Operation(hub_operation(operation)),
        HubEventKind::Account(state) => Event::Account(account_state(state)),
        HubEventKind::Unfunded => Event::Unfunded(proto::HubUnfunded {}),
        HubEventKind::Reduced { hold, amount } => Event::Reduced(proto::HubHoldReduced {
            hold: *hold,
            amount: *amount,
        }),
        HubEventKind::ClaimsPaid {
            hold,
            count,
            first,
            last,
            unlisted,
        } => Event::ClaimsPaid(proto::HubClaimsPaid {
            hold: *hold,
            count: *count,
            first_sequence: *first,
            last_sequence: *last,
            unlisted: *unlisted,
        }),
        HubEventKind::Resynced {
            height,
            timestamp_ms,
            holds_before,
            unread,
        } => Event::Resynced(proto::HubResynced {
            height: *height,
            timestamp_unix_ms: convert::time(*timestamp_ms),
            holds_before: *holds_before,
            unread: unread.map(purpose).unwrap_or_default(),
        }),
        HubEventKind::Dropped(dropped) => Event::Dropped(proto::HubDropped {
            purpose: purpose(*dropped),
        }),
        HubEventKind::Commitment(promise) => Event::Commitment(commitment(promise)),
        HubEventKind::Claim(promise) => Event::Claim(claim(promise)),
        HubEventKind::Move(moved) => Event::Move(move_event(moved)),
    };
    proto::HubEvent {
        scheme: event.scheme.to_string(),
        observed_at_unix_ms: convert::time(event.observed_at_ms),
        event: Some(kind),
    }
}
