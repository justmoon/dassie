//! Getting a certificate over ACME (RFC 8555), with the http-01 challenge.
//!
//! The daemon runs every order, the first as well as renewals. The original
//! ran the first order in the CLI and published its challenge tokens
//! through the daemon's RPC, which made two ACME clients and a token table
//! that threw on duplicates (spec 2 §4.5, §7, RT-13).
//!
//! The flow: load or create the account for the directory, order the
//! hostname, publish each http-01 key authorization in the node's challenge
//! table, tell the server it is ready, wait for the order, send a CSR for a
//! new ECDSA P-256 key, and fetch the chain. Tokens leave the table however
//! the order ends, including when it is cancelled.

use std::fmt;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use dassie_node_core::acme::{ChallengeTokens, DEFAULT_CHALLENGE_LIFETIME_MS};
use dassie_node_core::certificate::CertificateSource;
use dassie_node_core::config::{DirectoryUrl, Hostname};
use http::Request;
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, BodyWrapper, ChallengeType, HttpClient,
    Identifier, NewAccount, NewOrder, Order, OrderStatus, RetryPolicy,
};
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use crate::clock::Clock;
use crate::store::StoredCertificate;
use crate::tls;

/// How long to wait for the server to validate and to issue, each.
const ORDER_TIMEOUT: Duration = Duration::from_secs(120);

/// Most bytes read of a directory document.
const MAX_DIRECTORY_BYTES: usize = 64 * 1024;

/// A step of an order, for the progress the CLI shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Registering with the ACME server.
    CreatingAccount,
    /// Asking for a certificate.
    Ordering,
    /// Publishing the challenge.
    PreparingChallenge,
    /// Waiting for the server to fetch the challenge.
    Validating,
    /// Sending the certificate request.
    Finalizing,
    /// Fetching the certificate.
    Downloading,
}

/// An order failed.
#[derive(Debug)]
pub enum AcmeError {
    /// The request cannot work as it stands: no terms agreed, say.
    Precondition(String),
    /// The ACME server, or the way to it, failed or refused. `retry_after`
    /// is when the server suggests trying again, if it said.
    Server {
        /// What went wrong.
        message: String,
        /// When to try again, in ms from now.
        retry_after_ms: Option<u64>,
    },
    /// The account record or a key could not be handled.
    Internal(String),
}

impl fmt::Display for AcmeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Precondition(message) | Self::Internal(message) => f.write_str(message),
            Self::Server { message, .. } => write!(f, "the ACME server: {message}"),
        }
    }
}

impl std::error::Error for AcmeError {}

impl From<instant_acme::Error> for AcmeError {
    fn from(error: instant_acme::Error) -> Self {
        let retry_after_ms = match &error {
            instant_acme::Error::Timeout(Some(at)) => {
                let wait = at.saturating_duration_since(std::time::Instant::now());
                Some(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX))
            }
            _ => None,
        };
        Self::Server {
            message: error.to_string(),
            retry_after_ms,
        }
    }
}

type Https = HyperClient<HttpsConnector<HttpConnector>, BodyWrapper<Bytes>>;

/// The HTTP side of ACME: the system's roots, or only a test CA's.
#[derive(Clone)]
pub struct AcmeClient {
    http: Https,
}

impl fmt::Debug for AcmeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AcmeClient")
    }
}

impl AcmeClient {
    /// A client that trusts the system's roots, or, with `ca_file`, only the
    /// root in that PEM file (for test CAs such as Pebble).
    pub fn new(ca_file: Option<&Path>) -> Result<Self, AcmeError> {
        let internal = |error: &dyn fmt::Display| AcmeError::Internal(error.to_string());
        let builder = match ca_file {
            None => HttpsConnectorBuilder::new()
                .with_provider_and_platform_verifier(tls::provider())
                .map_err(|e| internal(&e))?,
            Some(path) => {
                let mut roots = RootCertStore::empty();
                for certificate in CertificateDer::pem_file_iter(path).map_err(|e| internal(&e))? {
                    roots
                        .add(certificate.map_err(|e| internal(&e))?)
                        .map_err(|e| internal(&e))?;
                }
                let config = rustls::ClientConfig::builder_with_provider(tls::provider())
                    .with_safe_default_protocol_versions()
                    .map_err(|e| internal(&e))?
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                HttpsConnectorBuilder::new().with_tls_config(config)
            }
        };
        let connector = builder.https_only().enable_http1().build();
        Ok(Self {
            http: HyperClient::builder(TokioExecutor::new()).build(connector),
        })
    }

    /// The terms of service URL in the directory's metadata, if it names one.
    pub async fn terms_of_service(
        &self,
        directory: &DirectoryUrl,
    ) -> Result<Option<String>, AcmeError> {
        let server = |message: String| AcmeError::Server {
            message,
            retry_after_ms: None,
        };
        let request = Request::get(directory.as_str())
            .body(BodyWrapper::default())
            .map_err(|e| server(e.to_string()))?;
        let mut response = HttpClient::request(&self.http, request).await?;
        if !response.parts.status.is_success() {
            return Err(server(format!(
                "the directory answered {}",
                response.parts.status
            )));
        }
        let body = response
            .body
            .into_bytes()
            .await
            .map_err(|e| server(e.to_string()))?;
        if body.len() > MAX_DIRECTORY_BYTES {
            return Err(server("the directory is unreasonably large".into()));
        }
        let document: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| server(format!("invalid directory: {e}")))?;
        Ok(document
            .pointer("/meta/termsOfService")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }

    /// Restores the account from stored credentials.
    pub async fn account(&self, credentials: &str) -> Result<Account, AcmeError> {
        let credentials: AccountCredentials = serde_json::from_str(credentials)
            .map_err(|e| AcmeError::Internal(format!("stored ACME account: {e}")))?;
        Ok(Account::builder_with_http(Box::new(self.http.clone()))
            .from_credentials(credentials)
            .await?)
    }

    /// Registers a new account, agreeing to the terms, and returns it with
    /// its credentials to store.
    pub async fn create_account(
        &self,
        directory: &DirectoryUrl,
        email: &str,
    ) -> Result<(Account, String), AcmeError> {
        let contact = format!("mailto:{email}");
        let contacts: Vec<&str> = if email.is_empty() {
            vec![]
        } else {
            vec![contact.as_str()]
        };
        let new_account = NewAccount {
            contact: &contacts,
            terms_of_service_agreed: true,
            only_return_existing: false,
        };
        let builder = Account::builder_with_http(Box::new(self.http.clone()));
        let (account, credentials) = builder
            .create(&new_account, directory.as_str().into(), None)
            .await?;
        let credentials = serde_json::to_string(&credentials)
            .map_err(|e| AcmeError::Internal(format!("ACME account: {e}")))?;
        Ok((account, credentials))
    }
}

/// Removes the tokens it published when dropped, so a failed or cancelled
/// order leaves nothing behind.
struct Published<'a> {
    table: &'a Mutex<ChallengeTokens>,
    tokens: Vec<String>,
}

impl Drop for Published<'_> {
    fn drop(&mut self) {
        let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
        for token in &self.tokens {
            table.remove(token);
        }
    }
}

/// What an order needs from the node.
pub struct OrderContext<'a> {
    /// The account to order with.
    pub account: &'a Account,
    /// The name to order.
    pub hostname: &'a Hostname,
    /// Where to publish http-01 tokens.
    pub challenges: &'a Mutex<ChallengeTokens>,
    /// For token expiry.
    pub clock: &'a dyn Clock,
}

/// Orders a certificate for the context's hostname, reporting each step.
pub async fn order_certificate(
    context: &OrderContext<'_>,
    progress: &mut (dyn FnMut(Step) + Send),
) -> Result<StoredCertificate, AcmeError> {
    progress(Step::Ordering);
    let identifiers = [Identifier::Dns(context.hostname.as_str().into())];
    let mut order = context
        .account
        .new_order(&NewOrder::new(&identifiers))
        .await?;
    progress(Step::PreparingChallenge);
    let mut published = Published {
        table: context.challenges,
        tokens: vec![],
    };
    publish_challenges(&mut order, context, &mut published).await?;
    progress(Step::Validating);
    let retries = RetryPolicy::new().timeout(ORDER_TIMEOUT);
    let status = order.poll_ready(&retries).await?;
    if status != OrderStatus::Ready {
        return Err(AcmeError::Server {
            message: format!(
                "the order ended as {status:?}; the server could not fetch the challenge"
            ),
            retry_after_ms: None,
        });
    }
    progress(Step::Finalizing);
    let internal = |error: rcgen::Error| AcmeError::Internal(error.to_string());
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(internal)?;
    let request = CertificateParams::new(vec![context.hostname.as_str().to_string()])
        .map_err(internal)?
        .serialize_request(&key)
        .map_err(internal)?;
    order.finalize_csr(request.der()).await?;
    progress(Step::Downloading);
    let chain_pem = order.poll_certificate(&retries).await?;
    drop(published);
    Ok(StoredCertificate {
        source: CertificateSource::Acme,
        chain_pem,
        key_pem: key.serialize_pem(),
    })
}

/// Publishes the http-01 key authorization of every pending authorization
/// and tells the server each is ready.
async fn publish_challenges(
    order: &mut Order,
    context: &OrderContext<'_>,
    published: &mut Published<'_>,
) -> Result<(), AcmeError> {
    let mut authorizations = order.authorizations();
    while let Some(authorization) = authorizations.next().await {
        let mut authorization = authorization?;
        if authorization.status != AuthorizationStatus::Pending {
            continue;
        }
        // The order outlives validation by far less than this; the table
        // forgets the token afterwards even if removing it failed somehow.
        let expires_at_ms = context
            .clock
            .now_ms()
            .saturating_add(DEFAULT_CHALLENGE_LIFETIME_MS);
        let mut challenge = authorization
            .challenge(ChallengeType::Http01)
            .ok_or_else(|| {
                AcmeError::Precondition("the ACME server offers no http-01 challenge".into())
            })?;
        let token = challenge.token.clone();
        let key_authorization = challenge.key_authorization();
        context
            .challenges
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                &token,
                key_authorization.as_str(),
                expires_at_ms,
                context.clock.now_ms(),
            )
            .map_err(|error| AcmeError::Server {
                message: format!("unusable challenge token: {error:?}"),
                retry_after_ms: None,
            })?;
        published.tokens.push(token);
        challenge.set_ready().await?;
    }
    Ok(())
}
