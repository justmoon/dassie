//! The market maker's side of the XRP Ledger: one WebSocket to a server,
//! a request at a time, each answered or timed out; the pool, read with
//! `amm_info`; and a rebalance, one Payment with `SendMax` that it signs,
//! submits and follows until a validated ledger holds it or it can no
//! longer apply (spec/areas/xrpl.md#amm-pools).
//!
//! The connection is made as the node's engine makes its ledgers'
//! (`crates/dassie-node/src/engine/links.rs`): `wss://` with the client's
//! TLS trust, `ws://` only where private addresses are allowed, names
//! resolved as for the node's requests. A connection that fails is
//! dropped, and made again at the next request.

use std::time::Duration;

use dassie_node::outbound::{OutboundClient, WebSocket};
use dassie_xrpl::Wallet;
use dassie_xrpl::account::TESTNET_NETWORK_ID;
use dassie_xrpl::amount::{Amount, Asset, IssuedAsset};
use dassie_xrpl::api::{self, ApiError, Message as ApiMessage, ServerState};
use dassie_xrpl::codec::{self, SendMaxTerms};
use dassie_xrpl::keys::AccountId;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use super::strategy::{MAX_FEE_DROPS, Pool, Rebalance};

/// Most bytes in one message from the server.
const MAX_MESSAGE_BYTES: usize = 1 << 20;
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a request may wait for its answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Most other messages read while waiting for an answer, such as a
/// stream's, before the connection is dropped.
const MAX_SKIPPED_MESSAGES: u32 = 64;
/// How many ledgers a rebalance may wait to be included in: about a
/// minute on the testnet.
const LEDGER_WINDOW: u32 = 15;
/// How often a rebalance asks whether it was validated.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Most times it asks, so it never waits longer than its window by much.
const MAX_POLLS: u32 = 60;
/// Network IDs up to this are left out of a transaction, as the ledger
/// requires.
const MAX_LEGACY_NETWORK_ID: u32 = 1024;

/// A connection to an XRPL server, made when a request needs it.
pub struct Xrpl {
    url: String,
    client: OutboundClient,
    socket: Option<WebSocket>,
    next_id: u64,
}

/// How a rebalance ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A validated ledger holds it, with this result; what it delivered if
    /// it succeeded.
    Validated {
        /// Its result code, `tesSUCCESS` if it paid.
        result: String,
        /// What it delivered.
        delivered: Option<Amount>,
        /// Its hash.
        hash: String,
    },
    /// The server refused it at once, so it never applies.
    Refused(String),
    /// Its last ledger passed without it: it never applies.
    Expired,
    /// It was not seen validated in time; it may still apply.
    Unknown,
}

impl Outcome {
    /// Whether it paid, or still may: validated with success, or not seen
    /// either way.
    #[must_use]
    pub fn may_have_paid(&self) -> bool {
        match self {
            Self::Validated { result, .. } => result == "tesSUCCESS",
            Self::Unknown => true,
            Self::Refused(_) | Self::Expired => false,
        }
    }
}

impl Xrpl {
    /// A client of the server at `url`, which connects at the first
    /// request.
    #[must_use]
    pub const fn new(url: String, client: OutboundClient) -> Self {
        Self {
            url,
            client,
            socket: None,
            next_id: 1,
        }
    }

    /// Sends the request `build` makes with its ID, and returns its
    /// answer.
    ///
    /// # Errors
    ///
    /// The server could not be reached or did not answer in time (the
    /// connection is then dropped), or it answered with an error.
    pub async fn request(&mut self, build: impl FnOnce(u64) -> Vec<u8>) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let request =
            String::from_utf8(build(id)).map_err(|e| Failure::Transport(e.to_string()))?;
        if self.socket.is_none() {
            let socket = tokio::time::timeout(CONNECT_TIMEOUT, self.connect())
                .await
                .unwrap_or_else(|_| Err("connecting timed out".into()))
                .map_err(Failure::Transport)?;
            self.socket = Some(socket);
        }
        let answer = match self.socket.as_mut() {
            Some(socket) => tokio::time::timeout(REQUEST_TIMEOUT, exchange(socket, request, id))
                .await
                .unwrap_or_else(|_| Err("the server did not answer in time".into())),
            None => Err("not connected".into()),
        };
        match answer {
            Ok(outcome) => outcome.map_err(Failure::Api),
            Err(problem) => {
                self.socket = None;
                Err(Failure::Transport(problem))
            }
        }
    }

    /// A WebSocket to the server, as the node connects to its ledgers.
    async fn connect(&self) -> Result<WebSocket, String> {
        self.client.websocket(&self.url, MAX_MESSAGE_BYTES).await
    }

    /// The server's state, if it is on the testnet. The keys a rebalance
    /// signs with are the nodes' testnet keys, whose accounts have the
    /// same addresses on every network: like the node's account, the
    /// market maker signs nothing for a server of another network, or one
    /// that names none (spec/decisions/real-money.md).
    ///
    /// # Errors
    ///
    /// It cannot be read, or the server is on another network.
    pub async fn testnet_state(&mut self) -> Result<ServerState, String> {
        let state = self
            .request(api::server_state)
            .await
            .and_then(|result| {
                api::server_state_result(&result)
                    .ok_or_else(|| Failure::Transport("a server_state it cannot read".into()))
            })
            .map_err(|failure| failure.to_string())?;
        if state.network_id != Some(TESTNET_NETWORK_ID) {
            return Err("the XRPL server is not on the XRP Ledger testnet".into());
        }
        Ok(state)
    }

    /// The pool of XRP and `token`, as of a validated ledger, with neither
    /// asset frozen.
    ///
    /// # Errors
    ///
    /// There is none, it cannot be read, or it is not so.
    pub async fn pool(&mut self, token: IssuedAsset) -> Result<Pool, String> {
        let asset = Asset::Token {
            currency: token.currency,
            issuer: token.issuer,
        };
        let result = self
            .request(|id| api::amm_info(id, Asset::Xrp, asset))
            .await
            .map_err(|failure| failure.to_string())?;
        let info = api::amm_info_result(&result).ok_or("an amm_info answer it cannot read")?;
        if !info.validated {
            return Err("an amm_info answer of a ledger not validated".into());
        }
        if info.asset_frozen || info.asset2_frozen {
            return Err("the pool's token is frozen".into());
        }
        match (info.amount, info.amount2) {
            (Amount::Xrp(drops), Amount::Token(held))
                if held.currency == token.currency && held.issuer == token.issuer =>
            {
                Ok(Pool {
                    drops,
                    value: held.value,
                    token,
                    trading_fee: info.trading_fee,
                })
            }
            _ => Err("an amm_info answer of another pool".into()),
        }
    }

    /// Signs `rebalance` with `wallet`, paying `destination`, submits it,
    /// and follows it until a validated ledger holds it, its last ledger
    /// passes, or its polls run out.
    ///
    /// # Errors
    ///
    /// It was not signed, or the server refused it with an error: nothing
    /// was sent that can apply. A submit whose answer was lost is followed
    /// as one that was answered.
    pub async fn rebalance(
        &mut self,
        wallet: &Wallet,
        destination: AccountId,
        rebalance: &Rebalance,
    ) -> Result<Outcome, String> {
        let state = self.testnet_state().await?;
        let fee = fee_drops(&state).ok_or("the ledger's fee is above what a rebalance pays")?;
        let account = wallet.account();
        let info = self
            .request(|id| api::account_info(id, account))
            .await
            .map_err(|failure| failure.to_string())?;
        let info = api::account_info_result(&info).ok_or("an account_info it cannot read")?;
        let first = state.validated_ledger.max(info.ledger_index);
        let last = first.saturating_add(LEDGER_WINDOW);
        let terms = SendMaxTerms {
            destination,
            amount: rebalance.deliver,
            send_max: rebalance.send_max,
            fee,
            sequence: info.sequence,
            last_ledger_sequence: last,
            network_id: state.network_id.filter(|id| *id > MAX_LEGACY_NETWORK_ID),
            memos: Vec::new(),
        };
        let signed =
            codec::sign_send_max(wallet, terms).map_err(|e| format!("cannot sign: {e:?}"))?;
        let submitted = match self.request(|id| api::submit(id, &signed.blob)).await {
            Ok(submitted) => submitted,
            // The server may have it, though its answer was lost: what
            // the ledger says of its hash decides.
            Err(Failure::Transport(problem)) => {
                tracing::warn!(target: super::TARGET, %problem, "no answer to a rebalance's submit");
                return Ok(self.follow(&signed.hash, first, last).await);
            }
            Err(failure @ Failure::Api(_)) => return Err(failure.to_string()),
        };
        let Some(engine) = api::submit_result(&submitted) else {
            return Ok(self.follow(&signed.hash, first, last).await);
        };
        // A malformed or failed one (tem…, tef…, tel…) never applies; one
        // that claims a fee (tec…) or waits (ter…) is decided by a
        // validated ledger.
        if engine.starts_with("tem") || engine.starts_with("tef") || engine.starts_with("tel") {
            return Ok(Outcome::Refused(engine));
        }
        Ok(self.follow(&signed.hash, first, last).await)
    }

    /// Asks after the transaction `hash`, which can be in ledgers `first`
    /// to `last`, until it is validated or can no longer be.
    async fn follow(&mut self, hash: &[u8; 32], first: u32, last: u32) -> Outcome {
        for _ in 0..MAX_POLLS {
            tokio::time::sleep(POLL_INTERVAL).await;
            match self.request(|id| api::tx(id, hash, first, last)).await {
                Ok(result) => {
                    if let Some(record) = api::tx_result(&result).filter(|record| record.validated)
                    {
                        let delivered = record
                            .delivered
                            .map(Amount::Xrp)
                            .or(record.delivered_token.map(Amount::Token));
                        return Outcome::Validated {
                            result: record.result,
                            delivered,
                            hash: dassie_xrpl::hex_upper(hash),
                        };
                    }
                }
                Err(Failure::Api(ApiError {
                    error,
                    searched_all: Some(true),
                    ..
                })) if error == "txnNotFound" => return Outcome::Expired,
                Err(_) => {}
            }
        }
        Outcome::Unknown
    }
}

/// What the ledger charges now, in drops: the reference fee scaled by the
/// load, rounded up; `None` above [`MAX_FEE_DROPS`].
#[must_use]
pub fn fee_drops(state: &ServerState) -> Option<u64> {
    let scaled = u128::from(state.base_fee) * u128::from(state.load_factor);
    let base = u128::from(state.load_base.max(1));
    let fee = u64::try_from(scaled.div_ceil(base)).ok()?;
    (fee <= MAX_FEE_DROPS).then_some(fee)
}

/// Why a request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The server was not reached, or did not answer.
    Transport(String),
    /// It answered with an error.
    Api(ApiError),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(problem) => write!(f, "the XRPL server: {problem}"),
            Self::Api(error) => write!(f, "the XRPL server answered {}", error.error),
        }
    }
}

/// Sends `request` and reads until the answer to `id`.
async fn exchange(
    socket: &mut WebSocket,
    request: String,
    id: u64,
) -> Result<Result<Value, ApiError>, String> {
    socket
        .send(Message::text(request))
        .await
        .map_err(|e| e.to_string())?;
    for _ in 0..MAX_SKIPPED_MESSAGES {
        let bytes = match socket.next().await {
            Some(Ok(Message::Text(text))) => text.as_bytes().to_vec(),
            Some(Ok(Message::Binary(bytes))) => bytes.to_vec(),
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Close(_))) | None => return Err("the server closed".into()),
            Some(Err(problem)) => return Err(problem.to_string()),
        };
        if let ApiMessage::Response {
            id: answered,
            outcome,
        } = api::parse(&bytes)
            && answered == id
        {
            return Ok(outcome);
        }
    }
    Err("too many messages without an answer".into())
}

#[cfg(test)]
mod tests {
    use dassie_node::fake_ledger::{Options, Server};
    use dassie_node::store::StoredCertificate;
    use dassie_node_core::certificate::CertificateSource;
    use dassie_xrpl::amount::TokenValue;
    use dassie_xrpl::codec::{Signing, Transact};
    use dassie_xrpl::fake::{DSUSD_CURRENCY, DSUSD_ISSUER, FEE_BASE};

    use super::super::strategy::{Side, to_amount};
    use super::*;

    const PEBBLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls");

    fn dsusd() -> IssuedAsset {
        IssuedAsset {
            currency: DSUSD_CURRENCY,
            issuer: DSUSD_ISSUER,
        }
    }

    async fn fake_ledger() -> Server {
        Server::start(Options {
            network: &dassie_xrpl::network::TESTNET,
            listen: ([127, 0, 0, 1], 0).into(),
            faucet: Some((
                ([127, 0, 0, 1], 0).into(),
                StoredCertificate {
                    source: CertificateSource::Manual,
                    chain_pem: std::fs::read_to_string(format!("{PEBBLE}/cert.pem")).unwrap(),
                    key_pem: std::fs::read_to_string(format!("{PEBBLE}/key.pem")).unwrap(),
                },
            )),
            close_every: Duration::from_millis(100),
        })
        .await
        .unwrap()
    }

    fn client() -> OutboundClient {
        OutboundClient::new(
            Some(std::path::Path::new(&format!("{PEBBLE}/root.pem"))),
            true,
        )
        .unwrap()
    }

    /// Posts `body` to the fake's issuer, at `path` under `/tokens/`.
    async fn ask(client: &OutboundClient, server: &Server, path: &str, body: &Value) {
        let url = format!("{}/{path}", server.tokens_url.as_deref().unwrap());
        let response = client
            .web(dassie_node::outbound::WebRequest {
                method: http::Method::POST,
                url,
                headers: vec![(http::header::CONTENT_TYPE, "application/json".into())],
                body: Some(body.to_string().into_bytes()),
                timeout_ms: 10_000,
                max_response_bytes: 4_096,
            })
            .await
            .unwrap();
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
    }

    async fn eventually(mut check: impl FnMut() -> bool) {
        for _ in 0..200 {
            if check() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("it never happened");
    }

    /// An account with XRP and a line to DSUSD's issuer, as a node's
    /// token scheme makes it.
    async fn holder(server: &Server, seed: u8, drops: u64) -> Wallet {
        let wallet = Wallet::from_seed([seed; 16]);
        server.ledger().deposit(wallet.account(), drops);
        eventually(|| server.ledger().balance(wallet.account()).is_some()).await;
        let sequence = server.ledger().ledger();
        let signed = codec::TrustSet {
            account: wallet.account(),
            limit: IssuedAsset::DSUSD.amount(TokenValue::MAX),
            fee: FEE_BASE,
            sequence: server_sequence(server, wallet.account()),
            last_ledger_sequence: Some(sequence + 20),
            flags: codec::TF_SET_NO_RIPPLE,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(wallet.public_key()),
        }
        .sign(&wallet);
        let submit = serde_json::json!({ "command": "submit", "tx_blob": dassie_xrpl::hex_upper(&signed.blob) });
        server.ledger().request(0, submit.to_string().as_bytes());
        eventually(|| {
            server
                .ledger()
                .token_balance(wallet.account(), IssuedAsset::DSUSD)
                .is_some()
        })
        .await;
        wallet
    }

    fn server_sequence(server: &Server, account: AccountId) -> u32 {
        let request =
            serde_json::json!({ "command": "account_info", "account": account.to_address() });
        let reply = server.ledger().request(0, request.to_string().as_bytes());
        let reply: Value = serde_json::from_slice(&reply).unwrap();
        u32::try_from(
            reply["result"]["account_data"]["Sequence"]
                .as_u64()
                .unwrap(),
        )
        .unwrap()
    }

    /// Has the issuer mint `value` DSUSD to `account`, and waits for it.
    async fn mint(client: &OutboundClient, server: &Server, account: AccountId, value: &str) {
        let body = serde_json::json!({
            "token": "DSUSD",
            "destination": account.to_address(),
            "value": value,
        });
        ask(client, server, "mint", &body).await;
        let minted = TokenValue::parse(value);
        eventually(|| server.ledger().token_balance(account, IssuedAsset::DSUSD) == minted).await;
    }

    /// The pool, once a ledger has made it.
    async fn read_pool(xrpl: &mut Xrpl) -> Pool {
        for _ in 0..100 {
            if let Ok(pool) = xrpl.pool(dsusd()).await {
                return pool;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no pool");
    }

    #[tokio::test]
    async fn it_reads_a_dsusd_pool_and_swaps_through_it() {
        let server = fake_ledger().await;
        let client = client();
        // The pool: 1000 XRP against 500 DSUSD, at a fee of 0.3 %.
        ask(
            &client,
            &server,
            "amm",
            &serde_json::json!({ "token": "DSUSD", "drops": "1000000000", "value": "500", "fee": 300 }),
        )
        .await;
        let mut xrpl = Xrpl::new(server.url.clone(), client.clone());
        let pool = read_pool(&mut xrpl).await;
        assert_eq!(pool.drops, 1_000_000_000);
        assert_eq!(pool.value, TokenValue::parse("500").unwrap());
        assert_eq!(pool.trading_fee, 300);

        // The token node's account holds DSUSD and pays the XRP node's
        // account XRP for it, through the pool: a rebalance.
        let paying = holder(&server, 1, 100_000_000).await;
        let paid = holder(&server, 2, 100_000_000).await;
        mint(&client, &server, paying.account(), "20").await;
        let send_max = to_amount(Side::Token, 10_000_000_000, dsusd()).unwrap();
        let deliver = Amount::Xrp(19_000_000);
        let before = server.ledger().balance(paid.account()).unwrap();
        let rebalance = Rebalance {
            from: Side::Token,
            deliver,
            send_max,
        };
        let outcome = xrpl
            .rebalance(&paying, paid.account(), &rebalance)
            .await
            .unwrap();
        let Outcome::Validated {
            result, delivered, ..
        } = outcome
        else {
            panic!("validated: {outcome:?}");
        };
        assert_eq!(result, "tesSUCCESS");
        assert_eq!(delivered, Some(deliver));
        assert_eq!(
            server.ledger().balance(paid.account()),
            Some(before + 19_000_000)
        );
        let left = server
            .ledger()
            .token_balance(paying.account(), IssuedAsset::DSUSD)
            .unwrap();
        // It spent what the pool asked, less than SendMax.
        let spent = TokenValue::parse("20").unwrap().checked_sub(left).unwrap();
        assert!(spent < TokenValue::parse("10").unwrap() && !spent.is_zero());

        // A SendMax below the pool's price fails on the ledger, and only
        // costs the fee.
        let greedy = Rebalance {
            from: Side::Token,
            deliver: Amount::Xrp(19_000_000),
            send_max: to_amount(Side::Token, 1_000_000_000, dsusd()).unwrap(),
        };
        let outcome = xrpl
            .rebalance(&paying, paid.account(), &greedy)
            .await
            .unwrap();
        assert!(
            matches!(&outcome, Outcome::Validated { result, .. } if result == "tecPATH_PARTIAL"),
            "{outcome:?}"
        );
        assert_eq!(
            server
                .ledger()
                .token_balance(paying.account(), IssuedAsset::DSUSD),
            Some(left)
        );
        server.stop();
    }

    #[tokio::test]
    async fn it_signs_nothing_for_a_server_of_another_network() {
        let server = fake_ledger().await;
        let client = client();
        ask(
            &client,
            &server,
            "amm",
            &serde_json::json!({ "token": "DSUSD", "drops": "1000000000", "value": "500", "fee": 0 }),
        )
        .await;
        let mut xrpl = Xrpl::new(server.url.clone(), client.clone());
        assert!(xrpl.testnet_state().await.is_ok());
        let paying = holder(&server, 1, 100_000_000).await;
        let paid = holder(&server, 2, 100_000_000).await;
        mint(&client, &server, paying.account(), "20").await;
        let rebalance = Rebalance {
            from: Side::Token,
            deliver: Amount::Xrp(1_000_000),
            send_max: to_amount(Side::Token, 10_000_000_000, dsusd()).unwrap(),
        };
        let before = server.ledger().balance(paid.account());
        // Mainnet's ID, and none.
        for network_id in [Some(0), None] {
            server.ledger().set_network_id(network_id);
            assert!(xrpl.testnet_state().await.is_err());
            let refused = xrpl.rebalance(&paying, paid.account(), &rebalance).await;
            assert!(refused.is_err(), "{refused:?}");
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(server.ledger().balance(paid.account()), before);
        server.stop();
    }

    #[test]
    fn a_rebalance_may_have_paid_until_the_ledger_says_otherwise() {
        let validated = |result: &str| Outcome::Validated {
            result: result.into(),
            delivered: None,
            hash: String::new(),
        };
        assert!(validated("tesSUCCESS").may_have_paid());
        assert!(Outcome::Unknown.may_have_paid());
        assert!(!validated("tecPATH_PARTIAL").may_have_paid());
        assert!(!Outcome::Expired.may_have_paid());
        assert!(!Outcome::Refused("tefPAST_SEQ".into()).may_have_paid());
    }

    #[test]
    fn the_fee_follows_the_load_up_to_a_bound() {
        let state = |load_factor| ServerState {
            validated_ledger: 1,
            close_time: 0,
            base_fee: 10,
            reserve_base: 1_000_000,
            reserve_inc: 200_000,
            load_factor,
            load_base: 256,
            network_id: Some(1),
        };
        assert_eq!(fee_drops(&state(256)), Some(10));
        assert_eq!(fee_drops(&state(300)), Some(12));
        assert_eq!(fee_drops(&state(256 * 10)), Some(100));
        assert_eq!(fee_drops(&state(256 * 11)), None);
    }
}
