//! Between the core's types and the contract's messages.

use dassie_node_core::certificate::{CertificateInfo, CertificateSource};
use dassie_node_core::config::{NodeConfig, Realm};

use super::proto;
use crate::acme::Step;
use crate::node::{Runtime, SessionView};

/// Milliseconds as the contract's signed integer.
pub fn time(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

pub fn config(config: &NodeConfig) -> proto::NodeConfig {
    proto::NodeConfig {
        realm: realm(config.realm).into(),
        hostname: config
            .hostname
            .as_ref()
            .map(|name| name.as_str().to_string())
            .unwrap_or_default(),
        alias: config.alias.as_str().into(),
        acme_directory_url: config.acme_directory.as_str().into(),
    }
}

pub const fn realm(realm: Realm) -> proto::Realm {
    match realm {
        Realm::Test => proto::Realm::Test,
        Realm::Live => proto::Realm::Live,
    }
}

pub fn certificate(info: &CertificateInfo) -> proto::Certificate {
    proto::Certificate {
        source: match info.source {
            CertificateSource::SelfSigned => proto::CertificateSource::SelfSigned,
            CertificateSource::Acme => proto::CertificateSource::Acme,
            CertificateSource::Manual => proto::CertificateSource::Manual,
        }
        .into(),
        names: info.names.clone(),
        not_before_unix_ms: time(info.not_before_ms),
        not_after_unix_ms: time(info.not_after_ms),
        leaf_sha256: info.leaf_sha256.to_vec(),
    }
}

pub const fn runtime(runtime: Runtime) -> proto::Runtime {
    match runtime {
        Runtime::Standalone => proto::Runtime::Standalone,
        Runtime::Systemd => proto::Runtime::Systemd,
    }
}

pub const fn step(step: Step) -> proto::AcmeStep {
    match step {
        Step::CreatingAccount => proto::AcmeStep::CreatingAccount,
        Step::Ordering => proto::AcmeStep::Ordering,
        Step::PreparingChallenge => proto::AcmeStep::PreparingChallenge,
        Step::Validating => proto::AcmeStep::Validating,
        Step::Finalizing => proto::AcmeStep::Finalizing,
        Step::Downloading => proto::AcmeStep::Downloading,
    }
}

pub fn session(view: &SessionView) -> proto::Session {
    proto::Session {
        id: view.record.id.to_hex(),
        created_at_unix_ms: time(view.record.times.created_at_ms),
        last_used_at_unix_ms: time(view.record.times.last_used_at_ms),
        expires_at_unix_ms: time(view.expires_at_ms),
        user_agent: view.record.user_agent.clone(),
        current: view.current,
    }
}

pub fn device(row: &crate::store::DeviceRow) -> proto::Device {
    proto::Device {
        id: row.id.as_str().to_string(),
        name: row.name.clone(),
        paired_at_unix_ms: time(row.paired_at_ms),
        last_seen_at_unix_ms: time(row.last_seen_at_ms),
    }
}
