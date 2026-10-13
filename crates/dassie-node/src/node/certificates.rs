//! The certificate: at start, after changes, from ACME and from the owner.

use std::fmt;
use std::sync::Arc;

use dassie_node_core::acme::{RenewalDecision, RenewalInput};
use dassie_node_core::certificate::{self, CertificateAction, CertificateInfo, CertificateSource};
use dassie_node_core::config::{DirectoryUrl, Hostname};
use tokio_util::sync::CancellationToken;

use super::{Node, NodeError, lock};
use crate::acme::{AcmeError, OrderContext, Step};
use crate::log::target;
use crate::store::{Store, StoreError, StoredCertificate};
use crate::tls::{self, LoadedCertificate, TlsError};

/// The certificate to serve at start: the stored one if it will do,
/// otherwise a new self-signed one, stored for next time.
pub(super) fn initial(
    store: &Store,
    hostname: Option<&Hostname>,
    now_ms: u64,
) -> Result<LoadedCertificate, NodeError> {
    let stored = match store.certificate()? {
        Some(stored) => match tls::load(&stored) {
            Ok(loaded) => Some(loaded),
            Err(error) => {
                tracing::warn!(target: target::ACME, %error, "the stored certificate is unusable");
                None
            }
        },
        None => None,
    };
    match certificate::decide(stored.as_ref().map(|loaded| &loaded.info), hostname, now_ms) {
        CertificateAction::Keep => {
            stored.ok_or(NodeError::Tls(TlsError::Pem("no certificate".into())))
        }
        CertificateAction::MakeSelfSigned => make_self_signed(store, hostname, now_ms),
    }
}

fn make_self_signed(
    store: &Store,
    hostname: Option<&Hostname>,
    now_ms: u64,
) -> Result<LoadedCertificate, NodeError> {
    let names = certificate::self_signed_names(hostname);
    let stored = tls::self_signed(&names, now_ms).map_err(NodeError::Tls)?;
    let loaded = tls::load(&stored).map_err(NodeError::Tls)?;
    store.set_certificate(&stored, now_ms)?;
    tracing::info!(target: target::ACME, ?names, "made a self-signed certificate");
    Ok(loaded)
}

/// Getting a certificate over ACME failed.
#[derive(Debug)]
pub enum ObtainError {
    /// Not possible as things stand.
    Precondition(String),
    /// The ACME server failed or refused, and may say when to try again,
    /// in ms from now.
    Unavailable(String, Option<u64>),
    /// The database or a key failed.
    Internal(String),
}

impl fmt::Display for ObtainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Precondition(message)
            | Self::Unavailable(message, _)
            | Self::Internal(message) => f.write_str(message),
        }
    }
}

impl From<AcmeError> for ObtainError {
    fn from(error: AcmeError) -> Self {
        match error {
            AcmeError::Precondition(message) => Self::Precondition(message),
            AcmeError::Server { retry_after_ms, .. } => {
                Self::Unavailable(error.to_string(), retry_after_ms)
            }
            AcmeError::Internal(message) => Self::Internal(message),
        }
    }
}

impl From<StoreError> for ObtainError {
    fn from(error: StoreError) -> Self {
        Self::Internal(error.to_string())
    }
}

/// What the owner asks for when obtaining a certificate.
#[derive(Clone, Debug)]
pub struct ObtainRequest {
    /// The account's contact, if one is created.
    pub email: String,
    /// Whether the owner agreed to the CA's terms.
    pub agree_to_terms: bool,
    /// Another directory than the configured one.
    pub directory: Option<DirectoryUrl>,
}

impl Node {
    /// Makes the stored certificate fit the current hostname: a new
    /// self-signed one if it no longer covers it (spec 7 §4.3).
    pub(super) fn refresh_certificate(&self) -> Result<(), NodeError> {
        let config = self.config();
        let info = self.certificate.info();
        let now_ms = self.now_ms();
        if certificate::decide(Some(&info), config.hostname.as_ref(), now_ms)
            == CertificateAction::MakeSelfSigned
        {
            let loaded = make_self_signed(&self.store, config.hostname.as_ref(), now_ms)?;
            self.certificate.replace(loaded);
        }
        Ok(())
    }

    /// Serves `stored` and keeps it.
    fn install(&self, stored: &StoredCertificate) -> Result<CertificateInfo, ObtainError> {
        let now_ms = self.now_ms();
        let loaded =
            tls::load_valid(stored, now_ms).map_err(|e| ObtainError::Internal(e.to_string()))?;
        self.store.set_certificate(stored, now_ms)?;
        let info = loaded.info.clone();
        self.certificate.replace(loaded);
        tracing::info!(target: target::ACME, names = ?info.names, "certificate installed");
        Ok(info)
    }

    /// Installs a certificate from elsewhere, after checking it (RT-12).
    pub fn set_certificate(
        &self,
        chain_pem: &str,
        key_pem: &str,
    ) -> Result<CertificateInfo, TlsError> {
        let stored = StoredCertificate {
            source: CertificateSource::Manual,
            chain_pem: chain_pem.into(),
            key_pem: key_pem.into(),
        };
        let now_ms = self.now_ms();
        let loaded = tls::load_valid(&stored, now_ms)?;
        self.store
            .set_certificate(&stored, now_ms)
            .map_err(|e| TlsError::Pem(e.to_string()))?;
        let info = loaded.info.clone();
        self.certificate.replace(loaded);
        Ok(info)
    }

    /// The configured directory, or `directory`, with its terms of service.
    pub async fn acme_terms(
        &self,
        directory: Option<DirectoryUrl>,
    ) -> Result<(DirectoryUrl, Option<String>), ObtainError> {
        let directory = directory.unwrap_or_else(|| self.config().acme_directory);
        let terms = self.acme.terms_of_service(&directory).await?;
        Ok((directory, terms))
    }

    /// Obtains and installs a certificate for the hostname, creating the ACME
    /// account if there is none for the directory. One order runs at a time.
    pub async fn obtain_certificate(
        &self,
        request: ObtainRequest,
        progress: &mut (dyn FnMut(Step) + Send),
    ) -> Result<CertificateInfo, ObtainError> {
        let Ok(_order) = self.acme_order.try_lock() else {
            return Err(ObtainError::Precondition(
                "another certificate order is running".into(),
            ));
        };
        let config = self.config();
        let hostname = config.hostname.clone().ok_or_else(|| {
            ObtainError::Precondition("set the hostname before ordering a certificate".into())
        })?;
        let directory = request.directory.clone().unwrap_or(config.acme_directory);
        let account = match self.store.acme_account(directory.as_str())? {
            Some(credentials) => self.acme.account(&credentials).await?,
            None if !request.agree_to_terms => {
                return Err(ObtainError::Precondition(
                    "the CA's terms of service must be agreed to".into(),
                ));
            }
            None => {
                progress(Step::CreatingAccount);
                let (account, credentials) =
                    self.acme.create_account(&directory, &request.email).await?;
                self.store.set_acme_account(
                    directory.as_str(),
                    &credentials,
                    &request.email,
                    self.now_ms(),
                )?;
                account
            }
        };
        let context = OrderContext {
            account: &account,
            hostname: &hostname,
            challenges: &self.challenges,
            clock: self.clock.as_ref(),
        };
        let stored = crate::acme::order_certificate(&context, progress).await?;
        let info = self.install(&stored)?;
        if request.directory.is_some() {
            let mut config = self.config();
            config.acme_directory = directory;
            self.store_config(config)
                .map_err(|e| ObtainError::Internal(e.to_string()))?;
        }
        lock(&self.renewal).succeeded(self.now_ms());
        *lock(&self.acme_last_error) = None;
        Ok(info)
    }

    /// When the renewal next checks, and why the last attempt failed.
    #[must_use]
    pub fn renewal_status(&self) -> (bool, u64, Option<String>) {
        let has_account = self
            .store
            .acme_account(self.config().acme_directory.as_str())
            .ok()
            .flatten()
            .is_some();
        (
            has_account,
            lock(&self.renewal).next_check_ms(),
            lock(&self.acme_last_error).clone(),
        )
    }

    /// Checks hourly whether to renew, and renews (spec 2 §7), until
    /// `cancel` fires.
    pub async fn run_renewal(self: Arc<Self>, cancel: CancellationToken) {
        loop {
            let decision = self.poll_renewal();
            let until_ms = match decision {
                RenewalDecision::Renew => {
                    self.renew().await;
                    continue;
                }
                RenewalDecision::Wait { until_ms } => until_ms,
            };
            let wait = std::time::Duration::from_millis(until_ms.saturating_sub(self.now_ms()));
            tokio::select! {
                () = cancel.cancelled() => return,
                () = self.renewal_wake.notified() => {}
                () = tokio::time::sleep(wait) => {}
            }
        }
    }

    fn poll_renewal(&self) -> RenewalDecision {
        let config = self.config();
        let info = self.certificate.info();
        let has_account = matches!(
            self.store.acme_account(config.acme_directory.as_str()),
            Ok(Some(_))
        );
        let input = RenewalInput {
            has_account,
            hostname: config.hostname.as_ref(),
            certificate: Some(&info),
        };
        lock(&self.renewal).poll(input, self.now_ms())
    }

    async fn renew(&self) {
        tracing::info!(target: target::ACME, "renewing the certificate");
        let request = ObtainRequest {
            email: String::new(),
            agree_to_terms: false,
            directory: None,
        };
        let result = self.obtain_certificate(request, &mut |_| {}).await;
        if let Err(error) = result {
            let retry_after_ms = match error {
                ObtainError::Unavailable(_, retry_after_ms) => retry_after_ms,
                _ => None,
            };
            let next_ms = lock(&self.renewal).failed(retry_after_ms, self.now_ms());
            tracing::warn!(target: target::ACME, %error, next_attempt_ms = next_ms, "renewal failed");
            *lock(&self.acme_last_error) = Some(error.to_string());
        }
    }

    /// Makes the renewal look again now, after the hostname changed.
    pub(super) fn wake_renewal(&self) {
        lock(&self.renewal).wake(self.now_ms());
        self.renewal_wake.notify_one();
    }
}
