//! HubMemberService (`proto/dassie/ui/v1/hub_member.proto`): the node's
//! account in its settlement hub, and the owner's moves between it and the
//! node's XRP Ledger testnet account (spec/areas/hubs.md#in-the-node).

use super::handlers::Reply;
use super::proto::{self, request::Method, response};
use super::{RpcError, convert};
use crate::engine::{Direction, Engine, HubHold, HubStatus};

/// Whether `method` is one of HubMemberService's.
pub(super) const fn is_hub(method: &Method) -> bool {
    matches!(
        method,
        Method::GetHubStatus(_)
            | Method::HubDeposit(_)
            | Method::HubWithdraw(_)
            | Method::SetHubAutomatic(_)
    )
}

/// Answers HubMemberService's methods.
pub(super) async fn dispatch(engine: &Engine, method: Method) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::GetHubStatus(_) => {
            let status = running(engine.hub_status().await?)?;
            response::Method::GetHubStatus(proto::GetHubStatusResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::HubDeposit(request) => {
            let status = engine.hub_deposit(request.amount).await?;
            response::Method::HubDeposit(proto::HubDepositResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::HubWithdraw(request) => {
            let status = engine.hub_withdraw(request.amount).await?;
            response::Method::HubWithdraw(proto::HubWithdrawResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::SetHubAutomatic(request) => {
            let status = running(engine.set_hub_automatic(request.automatic).await?)?;
            response::Method::SetHubAutomatic(proto::SetHubAutomaticResponse {
                status: Some(hub_status(&status)),
            })
        }
        _ => return Err(RpcError::internal("not a hub method")),
    };
    Ok(Reply::Unary(reply))
}

/// The status, while a hub scheme runs.
fn running(status: Option<HubStatus>) -> Result<HubStatus, RpcError> {
    status.ok_or_else(|| RpcError::precondition("the settlement hub scheme is not running"))
}

fn hub_status(status: &HubStatus) -> proto::HubStatus {
    let hex: String = status
        .hub_id
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let (validators, answering) = status.validators;
    proto::HubStatus {
        hub_id: hex,
        account: status.account,
        connection: super::ledger::connection(status.status).into(),
        balance: status.balance,
        reserve: status.reserve,
        deposited: status.deposited,
        withdrawn: status.withdrawn,
        paying: status.paying.iter().map(hold).collect(),
        taking: status.taking.iter().map(hold).collect(),
        floor: status.floor,
        ceiling: status.ceiling,
        automatic: status.automatic,
        paused_until_unix_ms: status.paused_until_ms.map(convert::time),
        moving: status.moving.map(|moving| proto::HubMove {
            direction: match moving.direction {
                Direction::Deposit => proto::hub_move::Direction::Deposit,
                Direction::Withdraw => proto::hub_move::Direction::Withdraw,
            }
            .into(),
            amount: moving
                .drops
                .saturating_mul(dassie_hub::member::UNITS_PER_DROP),
            by_owner: moving.owner,
        }),
        validators: u32::try_from(validators).unwrap_or(u32::MAX),
        validators_answering: u32::try_from(answering).unwrap_or(u32::MAX),
    }
}

fn hold(hold: &HubHold) -> proto::HubHold {
    proto::HubHold {
        peer: hold.peer.map(|node| node.to_string()).unwrap_or_default(),
        account: hold.account,
        amount: hold.amount,
    }
}
