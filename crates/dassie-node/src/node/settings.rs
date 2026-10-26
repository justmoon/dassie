//! The owner's configuration changes.

use std::fmt;

use dassie_node_core::config::{Alias, ConfigError, Hostname, NodeConfig, Ports, Realm};

use super::{Node, NodeError, Runtime, lock};
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
    /// Changes the configuration: reads it, applies `change` and stores the
    /// result, all under `config_writer`, so that two changes at once cannot
    /// undo each other. `change` returning `false` means nothing changed.
    pub(super) fn change_config(
        &self,
        change: impl FnOnce(&mut NodeConfig) -> Result<bool, SettingsError>,
    ) -> Result<(NodeConfig, bool), SettingsError> {
        let _writer = lock(&self.config_writer);
        let mut config = self.config();
        if !change(&mut config)? {
            return Ok((config, false));
        }
        self.store.set_config(&config)?;
        self.config.send_replace(config.clone());
        Ok((config, true))
    }

    /// Sets the public hostname. The certificate follows: a self-signed one
    /// is made for the new name, and the renewal orders a real one if an
    /// ACME account exists. Every URL uses the new name at once.
    ///
    /// The node's URL must be one peers take, with whatever port it gets:
    /// with a longer one, the node would run as a periphery node
    /// (spec/areas/network.md#periphery-nodes).
    pub fn set_hostname(&self, hostname: &str) -> Result<NodeConfig, SettingsError> {
        let hostname = Hostname::parse(hostname)?;
        if !dassie_peer::wire::is_valid_url(&format!("https://{hostname}:65535")) {
            return Err(ConfigError::HostnameTooLong.into());
        }
        let (config, changed) = self.change_config(|config| {
            let changed = config.hostname.as_ref() != Some(&hostname);
            config.hostname = Some(hostname);
            Ok(changed)
        })?;
        if changed {
            self.refresh_certificate()?;
            self.wake_renewal();
        }
        Ok(config)
    }

    /// Sets the realm; `live` is refused (spec/decisions/real-money.md),
    /// but by the dev switch (spec/decisions/mainnet-schemes.md). It takes
    /// effect when the engine starts again.
    pub fn set_realm(&self, realm: Realm) -> Result<NodeConfig, SettingsError> {
        let realm = realm.check_settable(self.live_realm())?;
        let (config, _) = self.change_config(|config| {
            config.realm = realm;
            Ok(true)
        })?;
        Ok(config)
    }

    /// Sets the standalone runtime's ports, and says whether a restart is
    /// needed for them to take effect.
    pub fn set_ports(&self, http: u32, https: u32) -> Result<(NodeConfig, bool), SettingsError> {
        let ports = Ports::new(http, https)?;
        let (config, _) = self.change_config(|config| {
            config.ports = ports;
            Ok(true)
        })?;
        let listening = (self.http_port, self.https_port);
        let restart_required = self.runtime == Runtime::Standalone
            && listening != (config.ports.http(), config.ports.https());
        Ok((config, restart_required))
    }

    /// Sets the alias.
    pub fn set_alias(&self, alias: &str) -> Result<NodeConfig, SettingsError> {
        let alias = Alias::parse(alias)?;
        let (config, _) = self.change_config(|config| {
            config.alias = alias;
            Ok(true)
        })?;
        Ok(config)
    }
}
