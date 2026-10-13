//! The owner's configuration changes (RT-3, RT-4).

use std::fmt;

use dassie_node_core::config::{Alias, ConfigError, Hostname, NodeConfig, Ports, Realm};

use super::{Node, NodeError, Runtime};
use crate::store::StoreError;

/// A configuration change was refused or failed.
#[derive(Debug)]
pub enum SettingsError {
    /// The value is invalid.
    Invalid(ConfigError),
    /// Storing it, or the certificate it required, failed.
    Failed(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => error.fmt(f),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

impl From<ConfigError> for SettingsError {
    fn from(error: ConfigError) -> Self {
        Self::Invalid(error)
    }
}

impl From<StoreError> for SettingsError {
    fn from(error: StoreError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl From<NodeError> for SettingsError {
    fn from(error: NodeError) -> Self {
        Self::Failed(error.to_string())
    }
}

impl Node {
    /// Stores `config` and makes it current.
    pub(super) fn store_config(&self, config: NodeConfig) -> Result<(), StoreError> {
        self.store.set_config(&config)?;
        self.config.send_replace(config);
        Ok(())
    }

    /// Sets the public hostname. The certificate follows: a self-signed one
    /// is made for the new name, and the renewal orders a real one if an
    /// ACME account exists. Every URL uses the new name at once (RT-3).
    pub fn set_hostname(&self, hostname: &str) -> Result<NodeConfig, SettingsError> {
        let hostname = Hostname::parse(hostname)?;
        let mut config = self.config();
        if config.hostname.as_ref() == Some(&hostname) {
            return Ok(config);
        }
        config.hostname = Some(hostname);
        self.store_config(config.clone())?;
        self.refresh_certificate()?;
        self.wake_renewal();
        Ok(config)
    }

    /// Sets the realm; `live` is refused (D8).
    pub fn set_realm(&self, realm: Realm) -> Result<NodeConfig, SettingsError> {
        let mut config = self.config();
        config.realm = realm.check_settable()?;
        self.store_config(config.clone())?;
        Ok(config)
    }

    /// Sets the standalone runtime's ports, and says whether a restart is
    /// needed for them to take effect.
    pub fn set_ports(&self, http: u32, https: u32) -> Result<(NodeConfig, bool), SettingsError> {
        let mut config = self.config();
        config.ports = Ports::new(http, https)?;
        self.store_config(config.clone())?;
        let listening = (self.http_port, self.https_port);
        let restart_required = self.runtime == Runtime::Standalone
            && listening != (config.ports.http(), config.ports.https());
        Ok((config, restart_required))
    }

    /// Sets the alias.
    pub fn set_alias(&self, alias: &str) -> Result<NodeConfig, SettingsError> {
        let mut config = self.config();
        config.alias = Alias::parse(alias)?;
        self.store_config(config.clone())?;
        Ok(config)
    }
}
