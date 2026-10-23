//! `dassie setup`: sets a node up from its owner's passphrase, with no
//! browser, for an owner who keeps the phrase on the node's own host, as a
//! headless node or a market maker's nodes must
//! (spec/areas/node.md#the-dassie-command,
//! spec/decisions/headless-setup.md).
//!
//! It does what the wallet does: it derives the node key and its login
//! proof from the phrase, and posts them with the setup token to
//! `/api/setup` (spec/areas/http.md#setup). So the node checks a setup from
//! here exactly as one from a browser, and the phrase never reaches the
//! node, only the key does. Then it waits for the engine and adds each
//! settlement scheme it is given that is not active.
//!
//! Running it again changes nothing, and a node set up with another phrase
//! is refused, untouched. Every call is bounded in time, every wait in
//! attempts, and each step prints one line.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node::tls;
use dassie_node_core::hex;
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

/// What `dassie setup` was asked to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The file that holds the owner's passphrase.
    pub passphrase_file: PathBuf,
    /// The settlement schemes the node should have, in order.
    pub schemes: Vec<Scheme>,
}

/// A settlement scheme to add, with its config.
#[derive(Debug, PartialEq, Eq)]
pub struct Scheme {
    /// Its ID, for example `stub`.
    pub id: String,
    /// Its config, JSON; empty for none.
    pub config: String,
}

/// Reads `dassie setup`'s flags.
pub fn args(parser: &mut lexopt::Parser) -> Result<Options, lexopt::Error> {
    let mut passphrase_file = None;
    let mut schemes: Vec<Scheme> = Vec::new();
    while let Some(argument) = parser.next()? {
        match argument {
            Long("passphrase-file") => passphrase_file = Some(PathBuf::from(parser.value()?)),
            Long("scheme") => {
                let scheme = scheme(&parser.value()?.string()?)?;
                if schemes.len() == MAX_SCHEMES {
                    return Err(format!("more than {MAX_SCHEMES} --scheme flags").into());
                }
                if schemes.iter().any(|known| known.id == scheme.id) {
                    return Err(format!("--scheme {} given twice", scheme.id).into());
                }
                schemes.push(scheme);
            }
            other => return Err(other.unexpected()),
        }
    }
    let passphrase_file = passphrase_file.ok_or("--passphrase-file is required")?;
    Ok(Options {
        passphrase_file,
        schemes,
    })
}

/// `<id>` or `<id>=<config>`: the config is everything after the first `=`.
fn scheme(text: &str) -> Result<Scheme, lexopt::Error> {
    let (id, config) = text.split_once('=').unwrap_or((text, ""));
    if id.is_empty() {
        return Err(format!("--scheme {text:?} names no scheme").into());
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

async fn setup(env: &Env, options: &Options) -> Result<(), Failure> {
    let key = passphrase::node_key_from_file(&options.passphrase_file)?;
    let id = key.node_id();
    println!(
        "The passphrase in {} is node {id}.",
        options.passphrase_file.display()
    );
    let mut client = tokio::time::timeout(CALL_TIMEOUT, Client::connect(env))
        .await
        .map_err(|_| "The Dassie daemon did not answer on its socket".to_owned())??;
    let before = basic_state(&mut client).await?;
    let claimed = is_uninitialized(&before) && claim(&mut client, &key).await?;
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
        println!("Set the node up as {id}.");
    } else {
        println!("The node is set up already as {id}.");
    }
    wait_for_engine(&mut client).await?;
    println!("The node's engine is running.");
    for scheme in &options.schemes {
        add_scheme(&mut client, scheme, &after.active_settlement_schemes).await?;
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

fn is_uninitialized(basic: &proto::GetBasicStateResponse) -> bool {
    basic.state == i32::from(proto::NodeState::Uninitialized)
}

/// Claims the node with `key`, as the wallet does. False if it was set up
/// meanwhile, by whoever.
async fn claim(client: &mut Client, key: &NodeKey) -> Result<bool, Failure> {
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
    let body = format!(
        r#"{{"setupAuthorizationToken":"{token}","rawDassieKeyHex":"{}","loginAuthorizationSignature":"{}"}}"#,
        hex::encode(key.as_bytes()),
        hex::encode(key.login_secret().as_bytes())
    );
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
            r#"xrpl-testnet={"a":"b=c"}"#,
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
                    id: "xrpl-testnet".into(),
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
        let unnamed = parse(&["--passphrase-file", "p", "--scheme", "={}"]);
        assert!(unnamed.unwrap_err().contains("names no scheme"));
        let mut many = vec!["--passphrase-file".to_owned(), "p".to_owned()];
        for n in 0..=MAX_SCHEMES {
            many.extend(["--scheme".to_owned(), format!("s{n}")]);
        }
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(parse(&many).unwrap_err().contains("more than"));
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
        assert_eq!(
            shown(b"Invalid setup authorization token"),
            "Invalid setup authorization token"
        );
        assert_eq!(shown(b"a\nb\x1b[31m"), "a b [31m");
        assert_eq!(shown(&[b'x'; 1_000]).len(), MAX_REFUSAL_CHARS);
    }
}
