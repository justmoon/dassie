//! The founding methods (`proto/dassie/owner/v1/hub_founding.proto`):
//! founding settlement hubs as their sponsor (spec/areas/hubs.md#founding-a-hub),
//! and the payout bundles the node keeps of each of its hubs
//! (spec/areas/hubs.md#payout-bundles).

use dassie_hub::founding::Stage;
use dassie_identity::NodeId;

use super::handlers::Reply;
use super::proto::{self, request::Method, response};
use super::{RpcError, convert};
use crate::hub::driver::hex;
use crate::hub::payouts::{self, PayoutError, PayoutStatus};
use crate::hub::sponsor::{self, FoundError, FoundRequest, FoundingStatus};
use crate::node::Node;

/// Whether `method` is one of the founding methods.
pub(super) const fn is_founding(method: &Method) -> bool {
    matches!(
        method,
        Method::FoundHub(_)
            | Method::DropHubFounding(_)
            | Method::ListHubFoundings(_)
            | Method::GetHubPayout(_)
    )
}

/// Answers the founding methods.
pub(super) async fn dispatch(node: &Node, method: Method) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::FoundHub(request) => {
            let status =
                sponsor::found(node, found_request(request)?).map_err(|error| match error {
                    FoundError::Invalid(why) => RpcError::invalid(why),
                    FoundError::Precondition(why) => RpcError::precondition(why),
                })?;
            response::Method::FoundHub(proto::FoundHubResponse {
                founding: Some(founding(&status)),
            })
        }
        Method::DropHubFounding(request) => {
            let nonce = nonce(&request.nonce)?;
            if !sponsor::abandon(node, &nonce).map_err(RpcError::internal)? {
                return Err(RpcError::not_found("no such founding"));
            }
            response::Method::DropHubFounding(proto::DropHubFoundingResponse {})
        }
        Method::ListHubFoundings(_) => {
            response::Method::ListHubFoundings(proto::ListHubFoundingsResponse {
                foundings: foundings(node),
            })
        }
        Method::GetHubPayout(request) => {
            let hub = hub_id(&request.hub_id)?;
            let payout = payouts::payout(node, hub)
                .await
                .map_err(|error| match error {
                    PayoutError::NoHub => {
                        RpcError::precondition("the settlement hub scheme is not running")
                    }
                    PayoutError::Unnamed => {
                        RpcError::invalid("the node is in several settlement hubs: name one")
                    }
                    PayoutError::NotFound => RpcError::not_found("the node is in no such hub"),
                    PayoutError::Store(error) => RpcError::internal(error),
                })?;
            response::Method::GetHubPayout(payout_message(&payout))
        }
        _ => return Err(RpcError::internal("not a founding method")),
    };
    Ok(Reply::Unary(reply))
}

/// The node's foundings, for `GetStatus` too.
pub(super) fn foundings(node: &Node) -> Vec<proto::HubFounding> {
    match sponsor::statuses(node) {
        Ok(statuses) => statuses.iter().map(founding).collect(),
        Err(error) => {
            tracing::error!(target: crate::log::target::RPC, %error, "cannot read the hub foundings");
            Vec::new()
        }
    }
}

fn found_request(request: proto::FoundHubRequest) -> Result<FoundRequest, RpcError> {
    let validators = request
        .validators
        .iter()
        .map(|invite| {
            let node = NodeId::parse(&invite.node)
                .map_err(|_| RpcError::invalid(format!("not a node ID: {:?}", invite.node)))?;
            Ok((node, invite.endpoint.clone()))
        })
        .collect::<Result<Vec<_>, RpcError>>()?;
    Ok(FoundRequest {
        validators,
        terms: request.terms,
        funding_drops: request.escrow_funding_drops,
        xrpl: Some(request.xrpl_server).filter(|url| !url.is_empty()),
    })
}

/// The hub a request names: its ID as 64 hex digits, as
/// `HubStatus.hub_id` gives it, or none, for the node's only hub, when
/// empty.
fn hub_id(text: &str) -> Result<Option<[u8; 32]>, RpcError> {
    if text.is_empty() {
        return Ok(None);
    }
    let invalid = || RpcError::invalid("a hub's ID is 64 hex digits");
    let bytes = dassie_xrpl::from_hex(text).ok_or_else(invalid)?;
    let id: [u8; 32] = bytes.try_into().map_err(|_| invalid())?;
    Ok(Some(id))
}

fn nonce(text: &str) -> Result<[u8; 32], RpcError> {
    let invalid = || RpcError::invalid("not a founding's nonce");
    let digit = |byte: u8| {
        char::from(byte)
            .to_digit(16)
            .and_then(|d| u8::try_from(d).ok())
    };
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if pairs.len() != 32 || !rest.is_empty() {
        return Err(invalid());
    }
    let mut nonce = [0_u8; 32];
    for (out, [high, low]) in nonce.iter_mut().zip(pairs) {
        *out = digit(*high)
            .zip(digit(*low))
            .map(|(high, low)| high * 16 + low)
            .ok_or_else(invalid)?;
    }
    Ok(nonce)
}

fn founding(status: &FoundingStatus) -> proto::HubFounding {
    let stage = match status.stage {
        Stage::Accepting => proto::HubFoundingStage::Accepting,
        Stage::Signing => proto::HubFoundingStage::Signing,
        Stage::Certifying => proto::HubFoundingStage::Certifying,
        Stage::Escrow => proto::HubFoundingStage::Escrow,
        Stage::Founded => proto::HubFoundingStage::Founded,
    };
    proto::HubFounding {
        nonce: hex(&status.nonce),
        stage: stage.into(),
        hub: status
            .hub
            .as_ref()
            .map(|(hub, _)| hex(hub.0.as_bytes()))
            .unwrap_or_default(),
        rules: status
            .hub
            .as_ref()
            .map(|(_, rules)| hex(rules))
            .unwrap_or_default(),
        escrow: status.escrow.to_address(),
        escrow_funded: status.escrow_payment.is_some(),
        escrow_set_up: status.escrow_set_up,
        validators: status
            .validators
            .iter()
            .map(|invited| proto::HubFoundingValidator {
                node: invited.node.as_str().into(),
                endpoint: invited.endpoint.clone(),
                accepted: invited.accepted,
                signed: invited.signed,
                running: invited.running,
                escrow_checked: invited.escrow_checked,
                refusal: invited
                    .refusal
                    .map(|refusal| refusal.describe().to_string())
                    .unwrap_or_default(),
            })
            .collect(),
        problem: status.problem.clone().unwrap_or_default(),
        started_unix_ms: convert::time(status.created_ms),
    }
}

fn payout_message(payout: &PayoutStatus) -> proto::GetHubPayoutResponse {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    proto::GetHubPayoutResponse {
        hub: hex(payout.hub.0.as_bytes()),
        bundles: payout
            .bundles
            .iter()
            .map(|(validator, height, transactions)| proto::HubPayoutBundle {
                validator: count(*validator),
                height: *height,
                transactions: count(*transactions),
            })
            .collect(),
        quorum: count(payout.quorum),
        ready_height: payout.ready.as_ref().map(|(height, _)| *height),
        transactions: payout
            .ready
            .as_ref()
            .map(|(_, blobs)| blobs.iter().map(|blob| hex(blob)).collect())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_read_as_hex() {
        assert_eq!(nonce(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert!(nonce("ab").is_err());
        assert!(nonce(&"zz".repeat(32)).is_err());
    }

    // A payout names its hub as the hub member's methods do: by the ID
    // `HubStatus.hub_id` gives, or by none for the node's only hub.
    #[test]
    fn a_payout_names_its_hub_by_its_id_in_hex() {
        assert_eq!(hub_id("").ok(), Some(None));
        let hex = "ab".repeat(32);
        assert_eq!(hub_id(&hex).ok(), Some(Some([0xab; 32])));
        assert_eq!(hub_id(&hex.to_uppercase()).ok(), Some(Some([0xab; 32])));
        for wrong in ["ab", "zz", &hex[1..], &format!("{hex}00")] {
            assert!(hub_id(wrong).is_err(), "{wrong}");
        }
    }
}
