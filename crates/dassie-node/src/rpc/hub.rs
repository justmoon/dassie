//! The hub member's methods (`proto/dassie/owner/v1/hub_member.proto`):
//! the node's account in each settlement hub it is in, and the owner's
//! moves between it and the owner book of the hub's asset
//! (spec/areas/hubs.md#in-the-node).

use super::handlers::Reply;
use super::proto::{self, request::Method, response};
use super::{RpcError, convert};
use crate::engine::{Direction, Engine, HubHold, HubStatus};

/// Whether `method` is one of the hub member's.
pub(super) const fn is_hub(method: &Method) -> bool {
    matches!(
        method,
        Method::GetHubStatus(_)
            | Method::HubDeposit(_)
            | Method::HubWithdraw(_)
            | Method::SetHubAutomatic(_)
            | Method::HubGiveUp(_)
    )
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
            let hub = hub_id(&request.hub_id)?;
            let status = engine.hub_deposit(hub, request.amount).await?;
            response::Method::HubDeposit(proto::HubDepositResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::HubWithdraw(request) => {
            let hub = hub_id(&request.hub_id)?;
            let status = engine.hub_withdraw(hub, request.amount).await?;
            response::Method::HubWithdraw(proto::HubWithdrawResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::SetHubAutomatic(request) => {
            let hub = hub_id(&request.hub_id)?;
            let status = engine.set_hub_automatic(hub, request.automatic).await?;
            response::Method::SetHubAutomatic(proto::SetHubAutomaticResponse {
                status: Some(hub_status(&status)),
            })
        }
        Method::HubGiveUp(request) => {
            let hub = hub_id(&request.hub_id)?;
            let status = engine.hub_give_up(hub).await?;
            response::Method::HubGiveUp(proto::HubGiveUpResponse {
                status: Some(hub_status(&status)),
            })
        }
        _ => return Err(RpcError::internal("not a hub method")),
    };
    Ok(Reply::Unary(reply))
}

/// The hub a request names: its ID as 64 hex digits, or none, for the
/// node's only hub, when empty.
fn hub_id(text: &str) -> Result<Option<[u8; 32]>, RpcError> {
    if text.is_empty() {
        return Ok(None);
    }
    let invalid = || RpcError::invalid("a hub's ID is 64 hex digits");
    let bytes = dassie_xrpl::from_hex(text).ok_or_else(invalid)?;
    let id: [u8; 32] = bytes.try_into().map_err(|_| invalid())?;
    Ok(Some(id))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A request names its hub by the ID `HubStatus.hub_id` gives, or by
    // none for the node's only hub.
    #[test]
    fn a_request_names_its_hub_by_its_id_in_hex() {
        assert_eq!(hub_id("").ok(), Some(None));
        let id = [0xab; 32];
        let hex: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hub_id(&hex).ok(), Some(Some(id)));
        assert_eq!(hub_id(&hex.to_uppercase()).ok(), Some(Some(id)));
        for wrong in ["ab", "zz", &hex[1..], &format!("{hex}00")] {
            assert!(hub_id(wrong).is_err(), "{wrong}");
        }
    }
}
