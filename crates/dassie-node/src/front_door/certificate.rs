//! The front door's certificate for `*.<domain>`, read from the files
//! certbot keeps, and read again when they change, so a renewal takes
//! effect without a restart. A certificate that does not load, or does not
//! name the wildcard, is never served: the one before stays.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dassie_node_core::certificate::CertificateSource;
use tokio_util::sync::CancellationToken;

use crate::log::target;
use crate::store::StoredCertificate;
use crate::tls::{self, CertificateSlot, LoadedCertificate, MAX_PEM_BYTES};

/// How often the files are read again: renewals come weeks ahead of expiry.
const RELOAD_EVERY: Duration = Duration::from_secs(60);

/// Where the certificate and its key are.
#[derive(Clone, Debug)]
pub struct CertificateFiles {
    /// The chain, PEM, leaf first.
    pub chain: PathBuf,
    /// The key, PEM.
    pub key: PathBuf,
}

impl CertificateFiles {
    /// What the files hold now.
    ///
    /// # Errors
    ///
    /// A file cannot be read, or is too long.
    pub fn read(&self) -> Result<StoredCertificate, String> {
        Ok(StoredCertificate {
            source: CertificateSource::Manual,
            chain_pem: read_pem(&self.chain)?,
            key_pem: read_pem(&self.key)?,
        })
    }

    /// The certificate the files hold, if it loads and names `*.<domain>`.
    ///
    /// # Errors
    ///
    /// What is wrong with it.
    pub fn load(&self, domain: &str) -> Result<(StoredCertificate, LoadedCertificate), String> {
        let stored = self.read()?;
        let loaded = tls::load(&stored).map_err(|error| error.to_string())?;
        let wildcard = format!("*.{domain}");
        if !loaded.info.names.contains(&wildcard) {
            return Err(format!("the certificate does not name {wildcard}"));
        }
        Ok((stored, loaded))
    }
}

fn read_pem(path: &PathBuf) -> Result<String, String> {
    let shown = path.display();
    let length = std::fs::metadata(path)
        .map_err(|error| format!("cannot read {shown}: {error}"))?
        .len();
    if length > u64::try_from(MAX_PEM_BYTES).unwrap_or(u64::MAX) {
        return Err(format!("{shown} is longer than a certificate"));
    }
    std::fs::read_to_string(path).map_err(|error| format!("cannot read {shown}: {error}"))
}

/// Serves what the files hold from the next handshake on, if it changed
/// since `current` and loads; returns what is served then.
pub fn reload(
    files: &CertificateFiles,
    domain: &str,
    slot: &CertificateSlot,
    current: StoredCertificate,
) -> StoredCertificate {
    match files.read() {
        Ok(stored) if stored == current => current,
        Ok(_) => match files.load(domain) {
            Ok((stored, loaded)) => {
                tracing::info!(target: target::FRONT_DOOR, names = ?loaded.info.names, "serving a new certificate");
                slot.replace(loaded);
                stored
            }
            Err(error) => {
                tracing::warn!(target: target::FRONT_DOOR, %error, "the new certificate is not served");
                current
            }
        },
        Err(error) => {
            tracing::warn!(target: target::FRONT_DOOR, %error, "the certificate files cannot be read");
            current
        }
    }
}

/// Reads the files again every [`RELOAD_EVERY`] until `shutdown`.
pub async fn keep_current(
    files: CertificateFiles,
    domain: String,
    slot: Arc<CertificateSlot>,
    mut current: StoredCertificate,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            () = tokio::time::sleep(RELOAD_EVERY) => {}
            () = shutdown.cancelled() => return,
        }
        let files = files.clone();
        let (domain, slot_for_task) = (domain.clone(), Arc::clone(&slot));
        let read =
            tokio::task::spawn_blocking(move || reload(&files, &domain, &slot_for_task, current));
        match read.await {
            Ok(served) => current = served,
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const NOW_MS: u64 = 1_800_000_000_000;

    fn write(dir: &std::path::Path, names: &[&str]) -> CertificateFiles {
        let names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        let stored = tls::self_signed(&names, NOW_MS).unwrap();
        let files = CertificateFiles {
            chain: dir.join("fullchain.pem"),
            key: dir.join("privkey.pem"),
        };
        std::fs::write(&files.chain, stored.chain_pem).unwrap();
        std::fs::write(&files.key, stored.key_pem).unwrap();
        files
    }

    #[test]
    fn only_a_wildcard_for_the_domain_is_served() {
        let dir = tempfile::tempdir().unwrap();
        let files = write(dir.path(), &["*.example.net", "example.net"]);
        assert!(files.load("example.net").is_ok());
        assert!(files.load("other.net").is_err());
        let files = write(dir.path(), &["a.example.net"]);
        assert!(files.load("example.net").is_err());
    }

    #[test]
    fn a_renewal_is_served_and_a_broken_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let files = write(dir.path(), &["*.example.net"]);
        let (stored, loaded) = files.load("example.net").unwrap();
        let slot = CertificateSlot::new(loaded);
        // Unchanged: nothing happens.
        let current = reload(&files, "example.net", &slot, stored.clone());
        assert_eq!(current, stored);
        // Renewed: served from now on.
        write(dir.path(), &["*.example.net", "example.net"]);
        let renewed = reload(&files, "example.net", &slot, current);
        assert_ne!(renewed, stored);
        assert_eq!(slot.info().names, ["*.example.net", "example.net"]);
        // Half written: the renewal stays.
        std::fs::write(&files.key, "-----BEGIN PRIVATE KEY-----\n").unwrap();
        let kept = reload(&files, "example.net", &slot, renewed.clone());
        assert_eq!(kept, renewed);
        assert_eq!(slot.info().names, ["*.example.net", "example.net"]);
    }
}
