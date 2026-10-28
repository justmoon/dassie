//! `dassie setup`: sets a node up from its owner's passphrase, with no
//! browser, for an owner who keeps the phrase on the node's own host, as a
//! headless node or a market maker's nodes must
//! (spec/areas/node.md#the-dassie-command,
//! spec/decisions/headless-setup.md).
//!
//! It does what the wallet does: it derives the node key from the phrase,
//! and posts it with the setup token to `/api/setup`
//! (spec/areas/http.md#setup). So the node checks a setup from
//! here exactly as one from a browser, and the phrase never reaches the
//! node, only the key does, with the realm the node runs, the test realm
//! unless `--realm` says otherwise: it is fixed then, for good
//! (spec/decisions/mainnet-schemes.md). Then it waits for the engine, sets
//! the config of each external asset it is given, and adds each settlement
//! scheme it is given that is not active. Then it joins the realm's default
//! settlement hub when the node offers one: the owner book the hub funds
//! from, peering with nobody, unless it is active, and the hub's scheme
//! (spec/decisions/hubs-only.md). In the test realm it does unless
//! `--no-hub`; in the live realm only with `--hub`, since on mainnet a node
//! joins a hub only once its owner turns hubs on
//! (spec/designs/settlement-hubs.md#11-choosing-a-hub).
//!
//! A phrase names its realm (spec/decisions/phrase-format.md), so it is
//! read as a phrase of that realm and refused as one of the other; one
//! that needs correcting, or that says it was made more than a day after
//! the host's clock, is refused too, since no one is here to confirm or to
//! set the clock.
//!
//! Running it again changes nothing, and a node set up with another phrase,
//! or in another realm than the phrase's, is refused, untouched. Every
//! call is bounded in time, every wait in attempts, and each step prints
//! one line.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node::tls;
use dassie_node_core::config::Realm;
use dassie_node_core::hex;
use dassie_node_core::setup::SetupRequest;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use lexopt::prelude::*;
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest as _, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::client::{CallError, Client};
use crate::passphrase;

/// Most `--scheme` flags.
const MAX_SCHEMES: usize = 16;
/// Most `--asset` flags.
const MAX_ASSETS: usize = 8;
/// Exit code for a node that is set up with another passphrase.
const OTHER_NODE: u8 = 3;
/// How long one call over the IPC socket may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long connecting to the node's HTTPS listener at one address, with
/// the TLS handshake, may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the setup request may take, from the first connection to the
/// answer's last byte.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Most bytes of the node's answer to the setup request read: a refusal's
/// sentence.
const MAX_ANSWER_BYTES: usize = 4 * 1024;
/// Most characters of a refusal shown.
const MAX_REFUSAL_CHARS: usize = 200;
/// Time between two polls while waiting for the engine or a scheme.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Polls before giving up on the engine or a scheme: a minute.
const MAX_POLLS: u32 = 300;
/// Bytes of a setup token, as hex.
const TOKEN_HEX_CHARS: usize = 2 * dassie_node_core::setup::SETUP_TOKEN_BYTES;
/// The realm's default settlement hub: the scheme over it, which the node
/// lists only while this build has a default hub of it built in
/// (spec/areas/hubs.md#in-the-node), and the owner book it funds from
/// (spec/areas/settlement.md#owner-books).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DefaultHub {
    /// The hub's scheme.
    scheme: &'static str,
    /// The owner book of its asset.
    book: &'static str,
}

impl DefaultHub {
    /// The default hub of `realm`, of XRP: on the testnet `hub-xrpl-testnet-xrp`,
    /// funded from `xrpl-testnet-xrp`; on mainnet `hub-xrpl-xrp`, funded from
    /// `xrpl-xrp`.
    const fn of(realm: Realm) -> Self {
        match realm {
            Realm::Test => Self {
                scheme: "hub-xrpl-testnet-xrp",
                book: dassie_xrpl::scheme::SCHEME_ID,
            },
            Realm::Live => Self {
                scheme: "hub-xrpl-xrp",
                book: dassie_xrpl::scheme::MAINNET_SCHEME_ID,
            },
        }
    }
}

/// What `dassie setup` was asked to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The file that holds the owner's passphrase.
    pub passphrase_file: PathBuf,
    /// The settlement schemes the node should have, in order.
    pub schemes: Vec<Scheme>,
    /// The external assets' configs, set before any scheme is added.
    pub assets: Vec<Scheme>,
    /// The realm the node is set up in, if `--realm` names one: the test
    /// realm otherwise.
    pub realm: Option<Realm>,
    /// Whether to leave the realm's default settlement hub alone
    /// (`--no-hub`), rather than join it.
    pub no_hub: bool,
    /// Whether to join the realm's default settlement hub (`--hub`): in the
    /// live realm, the owner's turning hubs on.
    pub hub: bool,
}

impl Options {
    /// Whether to join the default settlement hub of `realm`: in the test
    /// realm unless `--no-hub`, in the live realm only with `--hub`.
    const fn joins_hub(&self, realm: Realm) -> bool {
        match realm {
            Realm::Test => !self.no_hub,
            Realm::Live => self.hub,
        }
    }
}

/// A settlement scheme to add, or an external asset to configure, with
/// its config.
#[derive(Debug, PartialEq, Eq)]
pub struct Scheme {
    /// Its ID, for example `stub` or `xrpl-testnet-account/0`.
    pub id: String,
    /// Its config, JSON; empty for none.
    pub config: String,
}

/// Reads `dassie setup`'s flags.
pub fn args(parser: &mut lexopt::Parser) -> Result<Options, lexopt::Error> {
    let mut passphrase_file = None;
    let mut schemes: Vec<Scheme> = Vec::new();
    let mut assets: Vec<Scheme> = Vec::new();
    let mut realm = None;
    let mut no_hub = false;
    let mut hub = false;
    while let Some(argument) = parser.next()? {
        match argument {
            Long("passphrase-file") => passphrase_file = Some(PathBuf::from(parser.value()?)),
            Long("no-hub") => no_hub = true,
            Long("hub") => hub = true,
            Long("realm") => {
                let text = parser.value()?.string()?;
                let parsed = Realm::parse(&text)
                    .map_err(|_| format!("--realm takes test or live, not {text:?}"))?;
                realm = Some(parsed);
            }
            Long("scheme") => {
                let scheme = scheme("scheme", &parser.value()?.string()?)?;
                add_once("scheme", &mut schemes, scheme, MAX_SCHEMES)?;
            }
            Long("asset") => {
                let asset = scheme("asset", &parser.value()?.string()?)?;
                add_once("asset", &mut assets, asset, MAX_ASSETS)?;
            }
            other => return Err(other.unexpected()),
        }
    }
    let passphrase_file = passphrase_file.ok_or("--passphrase-file is required")?;
    if hub && no_hub {
        return Err("--hub and --no-hub ask for opposite things".into());
    }
    Ok(Options {
        passphrase_file,
        schemes,
        assets,
        realm,
        no_hub,
        hub,
    })
}

/// Adds what `--<flag>` names to `known`, at most `most` of them, and each
/// once.
fn add_once(
    flag: &str,
    known: &mut Vec<Scheme>,
    new: Scheme,
    most: usize,
) -> Result<(), lexopt::Error> {
    if known.len() == most {
        return Err(format!("more than {most} --{flag} flags").into());
    }
    if known.iter().any(|known| known.id == new.id) {
        return Err(format!("--{flag} {} given twice", new.id).into());
    }
    known.push(new);
    Ok(())
}

/// `<id>` or `<id>=<config>`, as `--<flag>` gives it: the config is
/// everything after the first `=`.
fn scheme(flag: &str, text: &str) -> Result<Scheme, lexopt::Error> {
    let (id, config) = text.split_once('=').unwrap_or((text, ""));
    if id.is_empty() {
        return Err(format!("--{flag} {text:?} names no {flag}").into());
    }
    Ok(Scheme {
        id: id.to_owned(),
        config: config.to_owned(),
    })
}

/// Why `dassie setup` stopped.
#[derive(Debug)]
enum Failure {
    /// The node is set up with another passphrase; nothing was changed.
    OtherNode(String),
    /// Anything else.
    Failed(String),
}

impl From<String> for Failure {
    fn from(problem: String) -> Self {
        Self::Failed(problem)
    }
}

impl From<CallError> for Failure {
    fn from(error: CallError) -> Self {
        Self::Failed(error.message)
    }
}

/// Runs `dassie setup`.
pub fn run(options: &Options) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    let env = Env::from_process();
    match runtime.block_on(setup(&env, options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::OtherNode(problem)) => {
            eprintln!("{problem}");
            ExitCode::from(OTHER_NODE)
        }
        Err(Failure::Failed(problem)) => {
            eprintln!("{problem}");
            ExitCode::FAILURE
        }
    }
}

/// The node key of the passphrase file, a phrase of `realm`. A phrase of
/// the other realm is valid, and the operator may only have left out
/// `--realm`, so the refusal says how to read it as its own.
fn node_key(options: &Options, realm: Realm) -> Result<NodeKey, String> {
    let phrase_realm = match realm {
        Realm::Test => dassie_identity::Realm::Test,
        Realm::Live => dassie_identity::Realm::Live,
    };
    passphrase::node_key_from_file(
        &options.passphrase_file,
        phrase_realm,
        passphrase::now_ms(),
    )
    .map_err(|refusal| match refusal {
        passphrase::Refusal::OtherRealm { message, realm } => format!(
            "{message}: pass --realm {realm} to set the node up in the {realm} realm. Nothing was \
             changed."
        ),
        passphrase::Refusal::Other(message) => message,
    })
}

async fn setup(env: &Env, options: &Options) -> Result<(), Failure> {
    let realm = options.realm.unwrap_or_default();
    let key = node_key(options, realm)?;
    let id = key.node_id();
    println!(
        "The passphrase in {} is node {id}.",
        options.passphrase_file.display()
    );
    let mut client = tokio::time::timeout(CALL_TIMEOUT, Client::connect(env))
        .await
        .map_err(|_| "The Dassie daemon did not answer on its socket".to_owned())??;
    let before = basic_state(&mut client).await?;
    // A node set up before runs its realm for good, and a phrase of the
    // other is not its owner's.
    if !is_uninitialized(&before) {
        let runs = node_realm(&mut client).await?;
        if runs != realm {
            return Err(Failure::Failed(format!(
                "The node runs the {} realm, and the passphrase in {} is a phrase of the {} \
                 realm: a node's realm is fixed when it is set up. Nothing was changed.",
                runs.as_str(),
                options.passphrase_file.display(),
                realm.as_str()
            )));
        }
    }
    let claimed = is_uninitialized(&before) && claim(&mut client, &key, realm).await?;
    drop(key);
    // Again after a claim, to see it took; and after one that found the
    // node set up meanwhile, to see by whom.
    let after = if is_uninitialized(&before) {
        basic_state(&mut client).await?
    } else {
        before
    };
    if is_uninitialized(&after) {
        return Err("The node did not take the setup, and is still not set up"
            .to_owned()
            .into());
    }
    if after.node_id != id.as_str() {
        return Err(Failure::OtherNode(format!(
            "The node is set up as {}, not as {id}, the node of the passphrase in {}. \
             Nothing was changed.",
            after.node_id,
            options.passphrase_file.display()
        )));
    }
    if claimed {
        println!("Set the node up as {id}, in the {} realm.", realm.as_str());
    } else {
        println!("The node is set up already as {id}.");
    }
    wait_for_engine(&mut client).await?;
    println!("The node's engine is running.");
    for asset in &options.assets {
        set_asset_config(&mut client, asset).await?;
    }
    for scheme in &options.schemes {
        add_scheme(&mut client, scheme, &after.active_settlement_schemes).await?;
    }
    settle_hub(&mut client, options, realm).await
}

/// Joins the default settlement hub of `realm` if the owner's flags say
/// so; otherwise, in the live realm without `--no-hub`, says how to join
/// it, while the node offers it.
async fn settle_hub(client: &mut Client, options: &Options, realm: Realm) -> Result<(), Failure> {
    let hub = DefaultHub::of(realm);
    if options.joins_hub(realm) {
        return join_default_hub(client, hub).await;
    }
    if !options.no_hub
        && matches!(
            hub_plan(hub, &list_schemes(client).await?),
            HubPlan::Join(_)
        )
    {
        println!(
            "In the live realm, the node joins a settlement hub only once its owner turns hubs \
             on: pass --hub to join the default one."
        );
    }
    Ok(())
}

/// The settlement schemes the node lists.
async fn list_schemes(client: &mut Client) -> Result<Vec<proto::SettlementSchemeInfo>, Failure> {
    match call(
        client,
        Method::ListSettlementSchemes(proto::ListSettlementSchemesRequest {}),
    )
    .await?
    {
        response::Method::ListSettlementSchemes(list) => Ok(list.schemes),
        _ => Err(unexpected()),
    }
}

/// How `dassie setup` joins the realm's default settlement hub, from the
/// schemes the node lists.
#[derive(Debug, PartialEq, Eq)]
enum HubPlan {
    /// The node offers no default hub: this build has none built in, or
    /// the node's realm has none.
    NoneOffered,
    /// The node runs the hub's scheme already, over the default hub or the
    /// one its config names.
    Joined,
    /// Add these schemes, in order: the owner book the hub funds from, if
    /// it is not active, then the hub's scheme, with no config, for the
    /// default hub.
    Join(Vec<Scheme>),
}

/// The plan for the default hub `hub`, from the node's
/// `ListSettlementSchemes`: it lists the hub's scheme while it offers the
/// default hub, or runs the scheme (spec/areas/node.md#the-dassie-command).
fn hub_plan(hub: DefaultHub, schemes: &[proto::SettlementSchemeInfo]) -> HubPlan {
    let listed = |id: &str| schemes.iter().find(|scheme| scheme.id == id);
    match listed(hub.scheme) {
        None => HubPlan::NoneOffered,
        Some(scheme) if scheme.active => HubPlan::Joined,
        Some(_) => {
            let mut steps = Vec::with_capacity(2);
            if !listed(hub.book).is_some_and(|book| book.active) {
                steps.push(Scheme {
                    id: hub.book.to_owned(),
                    config: String::new(),
                });
            }
            steps.push(Scheme {
                id: hub.scheme.to_owned(),
                config: String::new(),
            });
            HubPlan::Join(steps)
        }
    }
}

/// Joins the realm's default settlement hub `hub`, if the node offers it:
/// adds the owner book it funds from, unless active, then its scheme. Each
/// step prints one line.
async fn join_default_hub(client: &mut Client, hub: DefaultHub) -> Result<(), Failure> {
    match hub_plan(hub, &list_schemes(client).await?) {
        HubPlan::NoneOffered => {
            println!(
                "This build has no default settlement hub for the node's realm: joining none."
            );
        }
        HubPlan::Joined => {
            println!("The settlement scheme {} is active already.", hub.scheme);
        }
        HubPlan::Join(steps) => {
            for step in &steps {
                add_scheme(client, step, &[]).await?;
            }
        }
    }
    Ok(())
}

/// One call over the IPC socket, bounded in time.
async fn call(client: &mut Client, method: Method) -> Result<response::Method, CallError> {
    tokio::time::timeout(CALL_TIMEOUT, client.call(method))
        .await
        .unwrap_or_else(|_| {
            Err(CallError {
                code: None,
                message: format!(
                    "The daemon did not answer within {} s",
                    CALL_TIMEOUT.as_secs()
                ),
            })
        })
}

fn unexpected() -> Failure {
    Failure::Failed("The daemon answered something this CLI does not understand".into())
}

async fn basic_state(client: &mut Client) -> Result<proto::GetBasicStateResponse, Failure> {
    match call(
        client,
        Method::GetBasicState(proto::GetBasicStateRequest {}),
    )
    .await?
    {
        response::Method::GetBasicState(basic) => Ok(basic),
        _ => Err(unexpected()),
    }
}

/// The realm the node runs.
async fn node_realm(client: &mut Client) -> Result<Realm, Failure> {
    match call(client, Method::GetConfig(proto::GetConfigRequest {})).await? {
        response::Method::GetConfig(proto::GetConfigResponse {
            config: Some(config),
            ..
        }) => match proto::Realm::try_from(config.realm) {
            Ok(proto::Realm::Test) => Ok(Realm::Test),
            Ok(proto::Realm::Live) => Ok(Realm::Live),
            _ => Err(unexpected()),
        },
        _ => Err(unexpected()),
    }
}

fn is_uninitialized(basic: &proto::GetBasicStateResponse) -> bool {
    basic.state == i32::from(proto::NodeState::Uninitialized)
}

/// Claims the node with `key` in `realm`, as the wallet does. False if it
/// was set up meanwhile, by whoever.
async fn claim(client: &mut Client, key: &NodeKey, realm: Realm) -> Result<bool, Failure> {
    let url = match call(client, Method::GetSetupUrl(proto::GetSetupUrlRequest {})).await {
        Ok(response::Method::GetSetupUrl(setup)) => setup.url,
        Err(CallError {
            code: Some(proto::ErrorCode::FailedPrecondition),
            ..
        }) => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(unexpected()),
    };
    let token = url
        .rsplit_once("/setup/")
        .map(|(_, token)| token)
        .filter(|token| is_token(token))
        .ok_or_else(|| "The daemon's setup URL holds no setup token".to_owned())?;
    let listener = match call(client, Method::GetStatus(proto::GetStatusRequest {})).await? {
        response::Method::GetStatus(status) => Listener::of(&status)?,
        _ => return Err(unexpected()),
    };
    // The key is what the node stores in its database once set up; the
    // phrase and the master seed, which it never gets, are gone already.
    let body = serde_json::to_string(&SetupRequest {
        setup_token: token.into(),
        node_key: hex::encode(key.as_bytes()),
        realm: Some(realm.as_str().into()),
    })
    .map_err(|error| error.to_string())?;
    post_setup(&listener, body).await
}

/// The node's HTTPS listener, as the daemon describes it over the owner's
/// socket: its port, and the certificate it serves there.
struct Listener {
    port: u16,
    leaf_sha256: [u8; 32],
}

impl Listener {
    fn of(status: &proto::GetStatusResponse) -> Result<Self, String> {
        let port = u16::try_from(status.https_port)
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| format!("The daemon names no HTTPS port ({})", status.https_port))?;
        let leaf_sha256 = status
            .certificate
            .as_ref()
            .and_then(|certificate| <[u8; 32]>::try_from(certificate.leaf_sha256.as_slice()).ok())
            .ok_or_else(|| {
                "The daemon does not say which certificate it serves: restart it, so that it \
                 runs the version of this command"
                    .to_owned()
            })?;
        Ok(Self { port, leaf_sha256 })
    }
}

/// 64 lowercase hex digits: a token that is safe to put in JSON as it is.
fn is_token(text: &str) -> bool {
    text.len() == TOKEN_HEX_CHARS && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Posts `body` to `/api/setup` on the node's HTTPS listener on this host.
/// True once set up; false if the route is gone, which it is once the node
/// has an identity.
async fn post_setup(listener: &Listener, body: String) -> Result<bool, Failure> {
    let port = listener.port;
    let origin = format!("https://localhost:{port}");
    let exchange = async {
        let tls = connect_local(listener).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(|e| format!("No HTTP with the node at {origin}: {e}"))?;
        let driver = tokio::spawn(connection);
        let request = http::Request::post("/api/setup")
            .header(http::header::HOST, format!("localhost:{port}"))
            .header(http::header::ORIGIN, &origin)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| format!("Could not make the setup request: {e}"))?;
        let answer = async {
            let response = sender.send_request(request).await?;
            let status = response.status();
            let text = Limited::new(response.into_body(), MAX_ANSWER_BYTES)
                .collect()
                .await
                .map(|body| body.to_bytes())
                .unwrap_or_default();
            Ok::<_, hyper::Error>((status, text))
        }
        .await;
        driver.abort();
        answer.map_err(|e| format!("The setup request to {origin} failed: {e}"))
    };
    let (status, text) = tokio::time::timeout(SETUP_TIMEOUT, exchange)
        .await
        .map_err(|_| {
            format!(
                "The node did not answer the setup request within {} s",
                SETUP_TIMEOUT.as_secs()
            )
        })??;
    match status {
        http::StatusCode::OK => Ok(true),
        http::StatusCode::NOT_FOUND => Ok(false),
        other => Err(format!("The node refused setup ({other}): {}", shown(&text)).into()),
    }
}

/// What the node said, as one short line of plain text.
fn shown(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_REFUSAL_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A TLS connection to the node's listener on loopback, IPv4 first, then
/// IPv6, as the daemon binds every address of one or both, or `--listen`
/// one of them. Only a listener that proves it holds the key of the
/// certificate the daemon named counts: whatever else answers at an
/// address is passed over, and is sent nothing.
async fn connect_local(listener: &Listener) -> Result<TlsStream<TcpStream>, String> {
    let config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NodeCertificate(listener.leaf_sha256)))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from("localhost").map_err(|e| format!("TLS: {e}"))?;
    let addresses = [
        std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, listener.port)),
        std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, listener.port)),
    ];
    let mut problems = Vec::with_capacity(addresses.len());
    for address in addresses {
        let attempt = async {
            let stream = TcpStream::connect(address).await?;
            connector.connect(name.clone(), stream).await
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => problems.push(format!("{address}: {error}")),
            Err(_) => problems.push(format!("{address}: no answer")),
        }
    }
    Err(format!(
        "Cannot reach the node's HTTPS listener on this host ({}). `dassie setup` runs on \
         the node's own host, and needs the listener on loopback or on every address.",
        problems.join("; ")
    ))
}

/// Accepts exactly the certificate whose SHA-256 the daemon named over the
/// owner's socket, and checks that the handshake is signed with its key.
///
/// No CA can vouch for a node that has no hostname, and its self-signed
/// certificate names `localhost`, which anyone can put in one. The pin
/// does not depend on who holds the port: another user may bind a
/// loopback address the node leaves free, but cannot sign with the key of
/// the node's certificate. Only the node holds that key, and whoever can
/// read it from the node's database can read the node key there too
/// (spec/decisions/headless-setup.md).
#[derive(Debug)]
struct NodeCertificate([u8; 32]);

impl ServerCertVerifier for NodeCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if Sha256::digest(end_entity.as_ref()).as_slice() == self.0 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "not the certificate the daemon says it serves".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = tls::provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, certificate, signature, &algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = tls::provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, certificate, signature, &algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        tls::provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The node's ledgers, or none while its engine is not running: before it
/// has started after setup, or for a moment after a failure.
async fn ledgers(client: &mut Client) -> Result<Option<proto::ListLedgersResponse>, Failure> {
    match call(client, Method::ListLedgers(proto::ListLedgersRequest {})).await {
        Ok(response::Method::ListLedgers(list)) => Ok(Some(list)),
        Ok(_) => Err(unexpected()),
        Err(CallError {
            code: Some(proto::ErrorCode::Unavailable),
            ..
        }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Polls until the engine answers, a bounded number of times.
async fn wait_for_engine(client: &mut Client) -> Result<(), Failure> {
    for _ in 0..MAX_POLLS {
        if ledgers(client).await?.is_some() {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(Failure::Failed(format!(
        "The node's engine did not start within {} s: `dassie status` and the daemon's \
         log may say why",
        wait_seconds()
    )))
}

fn wait_seconds() -> u64 {
    POLL_INTERVAL.saturating_mul(MAX_POLLS).as_secs()
}

/// Sets the owner's config of the external asset `asset` names: the same
/// config again changes nothing (spec/areas/settlement.md#external-assets).
async fn set_asset_config(client: &mut Client, asset: &Scheme) -> Result<(), Failure> {
    let id = &asset.id;
    let method = Method::SetAssetConfig(proto::SetAssetConfigRequest {
        asset: id.clone(),
        config: asset.config.clone(),
    });
    match call(client, method).await {
        Ok(response::Method::SetAssetConfig(set)) if set.in_effect => {
            println!("Set the config of {id}.");
            Ok(())
        }
        Ok(response::Method::SetAssetConfig(_)) => {
            println!(
                "Set the config of {id}; the node takes it at its next start, since schemes \
                 settle through it now."
            );
            Ok(())
        }
        Ok(_) => Err(unexpected()),
        Err(error) => Err(Failure::Failed(format!(
            "Could not set the config of {id}: {}",
            error.message
        ))),
    }
}

/// Adds `scheme` unless it is among `active`, and polls, a bounded number
/// of times, until the node lists a ledger of it.
async fn add_scheme(
    client: &mut Client,
    scheme: &Scheme,
    active: &[String],
) -> Result<(), Failure> {
    let id = &scheme.id;
    if active.contains(id) {
        println!("The settlement scheme {id} is active already.");
        return Ok(());
    }
    let method = Method::AddSettlementScheme(proto::AddSettlementSchemeRequest {
        id: id.clone(),
        config: scheme.config.clone(),
    });
    match call(client, method).await {
        Ok(response::Method::AddSettlementScheme(_)) => {}
        Ok(_) => return Err(unexpected()),
        Err(error) => {
            return Err(Failure::Failed(format!(
                "Could not add the settlement scheme {id}: {}",
                error.message
            )));
        }
    }
    for _ in 0..MAX_POLLS {
        let listed = ledgers(client).await?.is_some_and(|list| {
            list.ledgers
                .iter()
                .any(|ledger| ledger.settlement_scheme == *id)
        });
        if listed {
            println!("Added the settlement scheme {id}.");
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(Failure::Failed(format!(
        "The node took the settlement scheme {id} but lists no ledger of it after {} s",
        wait_seconds()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<Options, String> {
        let mut parser = lexopt::Parser::from_args(words.iter().copied());
        args(&mut parser).map_err(|e| e.to_string())
    }

    #[test]
    fn flags() {
        let options = parse(&[
            "--passphrase-file",
            "/run/phrase",
            "--scheme",
            "stub",
            "--scheme",
            r#"xrpl-testnet-xrp={"a":"b=c"}"#,
            "--asset",
            r#"xrpl-testnet-account/0={"server":"wss://a"}"#,
        ])
        .unwrap();
        assert_eq!(options.passphrase_file, PathBuf::from("/run/phrase"));
        assert_eq!(
            options.schemes,
            [
                Scheme {
                    id: "stub".into(),
                    config: String::new()
                },
                Scheme {
                    id: "xrpl-testnet-xrp".into(),
                    config: r#"{"a":"b=c"}"#.into()
                },
            ]
        );
        assert!(
            parse(&["--scheme", "stub"])
                .unwrap_err()
                .contains("--passphrase-file")
        );
        let twice = parse(&[
            "--passphrase-file",
            "p",
            "--scheme",
            "stub",
            "--scheme",
            "stub=x",
        ]);
        assert!(twice.unwrap_err().contains("twice"));
        assert_eq!(
            options.assets,
            [Scheme {
                id: "xrpl-testnet-account/0".into(),
                config: r#"{"server":"wss://a"}"#.into()
            }]
        );
        let unnamed = parse(&["--passphrase-file", "p", "--scheme", "={}"]);
        assert!(unnamed.unwrap_err().contains("names no scheme"));
        let unnamed = parse(&["--passphrase-file", "p", "--asset", "={}"]);
        assert!(unnamed.unwrap_err().contains("names no asset"));
        let twice = parse(&[
            "--passphrase-file",
            "p",
            "--asset",
            "a/0",
            "--asset",
            "a/0={}",
        ]);
        assert!(twice.unwrap_err().contains("--asset a/0 given twice"));
        let mut many = vec!["--passphrase-file".to_owned(), "p".to_owned()];
        for n in 0..=MAX_SCHEMES {
            many.extend(["--scheme".to_owned(), format!("s{n}")]);
        }
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(parse(&many).unwrap_err().contains("more than"));
        // The realm: the test realm unless named.
        assert_eq!(options.realm, None);
        let live = parse(&["--passphrase-file", "p", "--realm", "live"]).unwrap();
        assert_eq!(live.realm, Some(Realm::Live));
        let main = parse(&["--passphrase-file", "p", "--realm", "main"]);
        assert!(main.unwrap_err().contains("test or live"));
    }

    // spec/designs/settlement-hubs.md#11-choosing-a-hub: in the test realm
    // the default hub is joined unless `--no-hub`; in the live realm only
    // with `--hub`, the owner's turning hubs on. Both at once are refused.
    #[test]
    fn the_default_hub_is_joined_per_realm() {
        let plain = parse(&["--passphrase-file", "p"]).unwrap();
        assert!(!plain.no_hub && !plain.hub);
        assert!(plain.joins_hub(Realm::Test));
        assert!(!plain.joins_hub(Realm::Live));
        let no_hub = parse(&["--passphrase-file", "p", "--no-hub"]).unwrap();
        assert!(no_hub.no_hub);
        assert!(!no_hub.joins_hub(Realm::Test) && !no_hub.joins_hub(Realm::Live));
        let hub = parse(&["--passphrase-file", "p", "--realm", "live", "--hub"]).unwrap();
        assert!(hub.hub);
        assert!(hub.joins_hub(Realm::Live) && hub.joins_hub(Realm::Test));
        assert!(parse(&["--passphrase-file", "p", "--no-hub=yes"]).is_err());
        assert!(parse(&["--passphrase-file", "p", "--hub=yes"]).is_err());
        let both = parse(&["--passphrase-file", "p", "--hub", "--no-hub"]);
        assert!(both.unwrap_err().contains("opposite"));
    }

    fn listed(id: &str, active: bool) -> proto::SettlementSchemeInfo {
        proto::SettlementSchemeInfo {
            id: id.into(),
            active,
            ..Default::default()
        }
    }

    fn scheme(id: &str, config: &str) -> Scheme {
        Scheme {
            id: id.into(),
            config: config.into(),
        }
    }

    // The node lists the hub's scheme only while it offers the default hub,
    // or runs the scheme; the plan adds the owner book it funds from first,
    // peering with nobody, unless it is active, and changes nothing the
    // second time.
    #[test]
    fn the_default_hub_is_joined_when_the_node_offers_one() {
        for realm in [Realm::Test, Realm::Live] {
            let default = DefaultHub::of(realm);
            let (book, hub) = (default.book, default.scheme);
            let plan = |schemes: &[proto::SettlementSchemeInfo]| hub_plan(default, schemes);
            assert_eq!(plan(&[]), HubPlan::NoneOffered);
            assert_eq!(
                plan(&[listed("stub", true), listed(book, false)]),
                HubPlan::NoneOffered
            );
            assert_eq!(
                plan(&[listed(book, false), listed(hub, false)]),
                HubPlan::Join(vec![scheme(book, ""), scheme(hub, "")])
            );
            assert_eq!(
                plan(&[listed(book, true), listed(hub, false)]),
                HubPlan::Join(vec![scheme(hub, "")])
            );
            // Run again, with what the first run added active; or a hub
            // scheme the owner added with rules of its own.
            assert_eq!(
                plan(&[listed(book, true), listed(hub, true)]),
                HubPlan::Joined
            );
        }
        // The other realm's hub is not this realm's.
        let live = DefaultHub::of(Realm::Live);
        let test = DefaultHub::of(Realm::Test);
        assert_eq!(
            hub_plan(live, &[listed(test.book, true), listed(test.scheme, false)]),
            HubPlan::NoneOffered
        );
    }

    // The IDs are the node's: the hub's scheme, and the owner book of the
    // hub's asset, each of the realm's ledgers: on mainnet `hub-xrpl-xrp`
    // funded from `xrpl-xrp`.
    #[test]
    fn the_default_hub_names_schemes_the_node_runs() {
        assert_eq!(
            DefaultHub::of(Realm::Live),
            DefaultHub {
                scheme: "hub-xrpl-xrp",
                book: "xrpl-xrp"
            }
        );
        for realm in [Realm::Test, Realm::Live] {
            let hub = DefaultHub::of(realm);
            for id in [hub.scheme, hub.book] {
                let info = dassie_node::engine::SCHEMES
                    .iter()
                    .find(|info| info.id == id)
                    .unwrap_or_else(|| panic!("{id}"));
                let (ledger, node) = (format!("{:?}", info.ledger.realm()), format!("{realm:?}"));
                assert_eq!(ledger, node, "{id}");
            }
        }
    }

    #[test]
    fn tokens() {
        assert!(is_token(&"0a".repeat(32)));
        assert!(!is_token(&"0A".repeat(32)));
        assert!(!is_token(&"0a".repeat(31)));
        assert!(!is_token(&format!("{}\"", "0".repeat(63))));
    }

    #[test]
    fn only_the_certificate_the_daemon_names_is_accepted() {
        let served = CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x01]);
        let other = CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x02]);
        let pinned = NodeCertificate(Sha256::digest(served.as_ref()).into());
        let check = |certificate: &CertificateDer<'_>| {
            pinned.verify_server_cert(
                certificate,
                &[],
                &ServerName::try_from("localhost").unwrap(),
                &[],
                UnixTime::now(),
            )
        };
        assert!(check(&served).is_ok());
        assert!(check(&other).is_err());
    }

    #[test]
    fn a_daemon_that_names_no_certificate_is_sent_nothing() {
        let mut status = proto::GetStatusResponse {
            https_port: 5101,
            certificate: Some(proto::Certificate {
                leaf_sha256: vec![7; 32],
                ..Default::default()
            }),
            ..Default::default()
        };
        let listener = Listener::of(&status).unwrap();
        assert_eq!((listener.port, listener.leaf_sha256), (5101, [7; 32]));
        status.certificate = Some(proto::Certificate::default());
        assert!(Listener::of(&status).is_err());
        status.certificate = None;
        assert!(Listener::of(&status).is_err());
        status.https_port = 0;
        assert!(Listener::of(&status).is_err());
    }

    #[test]
    fn refusals_are_one_short_line() {
        assert_eq!(shown(b"Invalid setup token"), "Invalid setup token");
        assert_eq!(shown(b"a\nb\x1b[31m"), "a b [31m");
        assert_eq!(shown(&[b'x'; 1_000]).len(), MAX_REFUSAL_CHARS);
    }
}
