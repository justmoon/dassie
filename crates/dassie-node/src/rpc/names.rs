//! The names methods (`proto/dassie/owner/v1/names.proto`): the names the owner
//! registers for the node and renews, and how each stands
//! (spec/areas/network.md#the-registrant).

use dassie_peer::names::NameStatus;
use dassie_peer::names::registrant::{AtRegistrar, State};

use super::handlers::{Items, Reply};
use super::ledger::{permit, source};
use super::live::{self, Follow};
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use crate::engine::{Engine, NamesView};
use crate::node::Node;

/// Whether `method` is one of the names methods.
pub(super) const fn is_names(method: &Method) -> bool {
    matches!(
        method,
        Method::RegisterName(_) | Method::RenewName(_) | Method::SubscribeNames(_)
    )
}

/// Answers the names methods.
pub(super) async fn dispatch(
    node: &Node,
    engine: &Engine,
    method: Method,
    items: &Items,
) -> Result<Reply, RpcError> {
    let reply = match method {
        Method::RegisterName(request) => {
            engine.register_name(request.name).await?;
            response::Method::RegisterName(proto::RegisterNameResponse {})
        }
        Method::RenewName(request) => {
            engine.renew_name(request.name).await?;
            response::Method::RenewName(proto::RenewNameResponse {})
        }
        Method::SubscribeNames(_) => {
            let _permit = permit(node)?;
            return live::values(
                source(engine),
                items,
                Follow::Network,
                || async { Ok(names(&engine.names().await?)) },
                stream_item::Method::SubscribeNames,
            )
            .await;
        }
        _ => return Err(RpcError::internal("not a names method")),
    };
    Ok(Reply::Unary(reply))
}

fn names(view: &NamesView) -> proto::NamesStatus {
    proto::NamesStatus {
        registrars: view.registrars.iter().map(ToString::to_string).collect(),
        frozen: view.frozen,
        names: view.names.iter().map(own_name).collect(),
        node: view.own.to_string(),
        off: view.off,
    }
}

fn own_name(status: &NameStatus) -> proto::OwnName {
    proto::OwnName {
        name: status.name.as_str().into(),
        state: state(status.state).into(),
        holder: status.holder.map(|id| id.to_string()).unwrap_or_default(),
        expires_at_unix_ms: status.expires_ms.map_or(0, convert::time),
        unseal_by_unix_ms: status.unseal_by_ms.map_or(0, convert::time),
        decided_at_unix_ms: status.decided_ms.map_or(0, convert::time),
        registrars: status.registrars.iter().map(at_registrar).collect(),
    }
}

const fn state(state: State) -> proto::NameState {
    match state {
        State::Committing => proto::NameState::Committing,
        State::Unsealing => proto::NameState::Unsealing,
        State::Waiting => proto::NameState::Waiting,
        State::Held => proto::NameState::Held,
        State::Lapsing => proto::NameState::Lapsing,
        State::Lost => proto::NameState::Lost,
        State::Split => proto::NameState::Split,
        State::NotHeld => proto::NameState::NotHeld,
        State::Abandoned => proto::NameState::Abandoned,
    }
}

fn at_registrar(at: &AtRegistrar) -> proto::NameAtRegistrar {
    proto::NameAtRegistrar {
        registrar: at.registrar.to_string(),
        receipt: at.receipt,
        unsealed: at.unsealed,
        refused_status: at.refused.map_or(0, u32::from),
        listed_holder: at
            .listed
            .map(|listing| listing.holder.to_string())
            .unwrap_or_default(),
        listed_expires_at_unix_ms: at
            .listed
            .map_or(0, |listing| convert::time(listing.expires_at_ms)),
        list_fresh: at.fresh,
    }
}
