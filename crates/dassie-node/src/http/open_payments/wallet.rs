//! Wallet addresses and their key sets.

use dassie_payments::resources::{DassieWallet, WalletAddress};
use hyper::header::{self, HeaderValue};
use hyper::{Response, StatusCode};

use super::{error, json};
use crate::http::response::Body;
use crate::node::Node;
use crate::open_payments::auth::OpError;
use crate::open_payments::{Urls, WalletAddresses};

/// `GET /.well-known/pay`, `GET /pay/<slug>`: the wallet address, with
/// the URL it was asked for as its ID, and the node's ID for Dassie-aware
/// clients.
pub async fn wallet_address(node: &Node, urls: &Urls, path: &str) -> Response<Body> {
    let Ok(addresses) = WalletAddresses::of(node).await else {
        return error(&OpError::unavailable(), None);
    };
    let Some(ledger) = addresses.ledger_of_path(path) else {
        return error(&OpError::not_found("no such wallet address"), None);
    };
    let asset = ledger.asset();
    let alias = node.config().alias;
    let address = WalletAddress {
        id: format!("{}{path}", urls.origin()),
        public_name: (!alias.as_str().is_empty()).then(|| alias.as_str().to_string()),
        asset_code: asset.code.into(),
        asset_scale: asset.scale,
        auth_server: urls.auth_server(),
        resource_server: urls.origin().to_string(),
        dassie: node.node_key().map(|key| DassieWallet {
            node_id: key.node_id().to_string(),
        }),
    };
    let mut response = json(StatusCode::OK, &address);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("max-age=60"),
    );
    response
}

/// `GET <wallet address>/jwks.json`: the node's key, which signs its
/// requests to other wallets.
pub async fn jwks(node: &Node, path: &str) -> Response<Body> {
    let Ok(addresses) = WalletAddresses::of(node).await else {
        return error(&OpError::unavailable(), None);
    };
    if addresses.ledger_of_path(path).is_none() {
        return error(&OpError::not_found("no such wallet address"), None);
    }
    let key = node
        .node_key()
        .and_then(|key| node.open_payments().client_key(&key).map(|key| key.jwks()));
    match key {
        Some(jwks) => {
            let mut response = json(StatusCode::OK, &jwks);
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=300"),
            );
            response
        }
        None => error(&OpError::unavailable(), None),
    }
}
