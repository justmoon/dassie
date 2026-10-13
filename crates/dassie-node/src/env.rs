//! The daemon's environment, read once at start.
//!
//! The variable names and their precedence are the original's (spec 2 §3.3),
//! minus those that no longer mean anything: `NODE_ENV`, `DEBUG_HIDE_DATE`,
//! the cache and temporary directories, which nothing used (RT-4). The
//! original evaluated them lazily, so a bad value killed whichever actor
//! first touched it and left the daemon half-started (RT-24). Here every
//! variable is checked before anything starts, and a bad one stops the
//! daemon with a message that names it.
//!
//! The environment arrives as an [`Env`] snapshot rather than being read
//! through `std::env`, so tests can pass their own (D16).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use dassie_identity::{NodeId, PublicKey, base64url};
use serde::Deserialize;

use crate::log::{LogFilter, LogLevel};

/// The bootstrap nodes of the original's test network (spec 2 §17), whose
/// IDs are checked against their keys like any other list (NET-19).
pub const DEFAULT_BOOTSTRAP_NODES: &str = r#"[
  {"id":"dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW","url":"https://test1.dassie.xyz","publicKey":"QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"},
  {"id":"dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U","url":"https://test2.dassie.xyz","publicKey":"tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ"}
]"#;

/// Length of `DASSIE_DEV_SECURITY_TOKEN`.
pub const DEV_SECURITY_TOKEN_LENGTH: usize = 64;

/// Most bootstrap nodes accepted.
pub const MAX_BOOTSTRAP_NODES: usize = 64;

/// A snapshot of environment variables.
#[derive(Clone, Debug, Default)]
pub struct Env(BTreeMap<String, OsString>);

impl Env {
    /// The process's environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self(
            std::env::vars_os()
                .filter_map(|(name, value)| Some((name.into_string().ok()?, value)))
                .collect(),
        )
    }

    /// An environment of exactly these variables.
    #[must_use]
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        )
    }

    /// A variable as text, if set. Not being UTF-8 is an error.
    pub fn get(&self, name: &'static str) -> Result<Option<&str>, EnvError> {
        match self.0.get(name) {
            None => Ok(None),
            Some(value) => value.to_str().map(Some).ok_or(EnvError {
                variable: name,
                problem: "is not valid UTF-8".into(),
            }),
        }
    }

    /// A variable as a path, if set and not empty.
    #[must_use]
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.0
            .get(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }
}

/// A variable has a value the daemon cannot use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvError {
    /// Which one.
    pub variable: &'static str,
    /// What is wrong with it.
    pub problem: String,
}

impl fmt::Display for EnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.variable, self.problem)
    }
}

impl std::error::Error for EnvError {}

/// A node the daemon asks about the network ([3 §11](../../spec)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapNode {
    /// Its node ID, which matches its key.
    pub id: NodeId,
    /// Its HTTPS URL.
    pub url: String,
    /// Its Ed25519 key.
    pub public_key: PublicKey,
}

/// Everything the daemon takes from its environment.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where the database lives.
    pub data_dir: PathBuf,
    /// The IPC socket, if given explicitly.
    pub ipc_socket_path: Option<PathBuf>,
    /// What reaches standard output.
    pub log_filter: LogFilter,
    /// A token that authenticates RPC sockets in development. Read and
    /// checked now; used from M5.
    pub dev_security_token: Option<String>,
    /// The network's bootstrap nodes. Read and checked now; used from M4.
    pub bootstrap_nodes: Vec<BootstrapNode>,
    /// An extra root the ACME client trusts, for test CAs such as Pebble.
    pub acme_ca_file: Option<PathBuf>,
}

impl Settings {
    /// Reads and checks every variable.
    pub fn from_env(env: &Env) -> Result<Self, EnvError> {
        Ok(Self {
            data_dir: data_dir(env)?,
            ipc_socket_path: env.path("DASSIE_IPC_SOCKET_PATH"),
            log_filter: log_filter(env)?,
            dev_security_token: dev_security_token(env)?,
            bootstrap_nodes: bootstrap_nodes(env)?,
            acme_ca_file: env.path("DASSIE_ACME_CA_FILE"),
        })
    }
}

/// `DASSIE_STATE_DIRECTORY`, then systemd's `STATE_DIRECTORY` (its first
/// entry), then `$XDG_DATA_HOME/dassie`, then `~/.local/share/dassie`.
fn data_dir(env: &Env) -> Result<PathBuf, EnvError> {
    if let Some(path) = env.path("DASSIE_STATE_DIRECTORY") {
        return Ok(path);
    }
    if let Some(list) = env.get("STATE_DIRECTORY")?
        && let Some(first) = list.split(':').find(|entry| !entry.is_empty())
    {
        return Ok(PathBuf::from(first));
    }
    if let Some(path) = env.path("XDG_DATA_HOME") {
        return Ok(path.join("dassie"));
    }
    match env.path("HOME") {
        Some(home) => Ok(home.join(".local/share/dassie")),
        None => Err(EnvError {
            variable: "DASSIE_STATE_DIRECTORY",
            problem: "is not set, and neither are STATE_DIRECTORY, XDG_DATA_HOME or HOME".into(),
        }),
    }
}

fn log_filter(env: &Env) -> Result<LogFilter, EnvError> {
    let level = match env.get("DASSIE_LOG_LEVEL")? {
        None => LogLevel::Info,
        Some(text) => LogLevel::parse(text).ok_or(EnvError {
            variable: "DASSIE_LOG_LEVEL",
            problem: format!("must be debug, info, warn, error or none, not {text:?}"),
        })?,
    };
    Ok(LogFilter::new(level, env.get("DEBUG")?.unwrap_or_default()))
}

fn dev_security_token(env: &Env) -> Result<Option<String>, EnvError> {
    match env.get("DASSIE_DEV_SECURITY_TOKEN")? {
        None => Ok(None),
        Some(token) if token.len() == DEV_SECURITY_TOKEN_LENGTH && token.is_ascii() => {
            Ok(Some(token.into()))
        }
        Some(_) => Err(EnvError {
            variable: "DASSIE_DEV_SECURITY_TOKEN",
            problem: format!("must be {DEV_SECURITY_TOKEN_LENGTH} ASCII characters"),
        }),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BootstrapNodeJson {
    id: String,
    url: String,
    public_key: String,
}

fn bootstrap_nodes(env: &Env) -> Result<Vec<BootstrapNode>, EnvError> {
    let text = env
        .get("DASSIE_BOOTSTRAP_NODES")?
        .unwrap_or(DEFAULT_BOOTSTRAP_NODES);
    let error = |problem: String| EnvError {
        variable: "DASSIE_BOOTSTRAP_NODES",
        problem,
    };
    let nodes: Vec<BootstrapNodeJson> =
        serde_json::from_str(text).map_err(|e| error(format!("is not valid: {e}")))?;
    if nodes.len() > MAX_BOOTSTRAP_NODES {
        return Err(error(format!(
            "lists more than {MAX_BOOTSTRAP_NODES} nodes"
        )));
    }
    nodes
        .into_iter()
        .map(|node| {
            let checked = check_bootstrap_node(&node);
            checked.ok_or_else(|| error(format!("has an invalid entry for {:?}", node.id)))
        })
        .collect()
}

/// A bootstrap node's entry, if its URL is HTTPS, its key decodes and its
/// ID is the one the key gives.
fn check_bootstrap_node(node: &BootstrapNodeJson) -> Option<BootstrapNode> {
    let mut key = [0; 32];
    let length = base64url::decode(node.public_key.as_bytes(), &mut key).ok()?;
    if length != key.len() || !node.url.starts_with("https://") {
        return None;
    }
    let public_key = PublicKey::from_bytes(&key).ok()?;
    let id = NodeId::parse(&node.id).ok()?;
    (public_key.node_id() == id).then(|| BootstrapNode {
        id,
        url: node.url.clone(),
        public_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> Result<Settings, EnvError> {
        Settings::from_env(&Env::from_pairs(pairs.iter().copied()))
    }

    #[test]
    fn data_directory_precedence() {
        let dir = |pairs: &[(&str, &str)]| settings(pairs).map(|s| s.data_dir);
        let all = [
            ("DASSIE_STATE_DIRECTORY", "/a"),
            ("STATE_DIRECTORY", "/b:/c"),
            ("XDG_DATA_HOME", "/d"),
            ("HOME", "/e"),
        ];
        assert_eq!(dir(&all), Ok("/a".into()));
        assert_eq!(dir(&all[1..]), Ok("/b".into()));
        assert_eq!(dir(&all[2..]), Ok("/d/dassie".into()));
        assert_eq!(dir(&all[3..]), Ok("/e/.local/share/dassie".into()));
        assert!(dir(&[]).is_err());
    }

    #[test]
    fn the_default_bootstrap_nodes_check_out() {
        let nodes = settings(&[("HOME", "/h")]).map(|s| s.bootstrap_nodes.len());
        assert_eq!(nodes, Ok(2));
    }

    // RT-24: in the original these broke whichever actor read them first,
    // and the daemon ran on half-started.
    #[test]
    fn bad_values_are_errors_that_name_the_variable() {
        let bad = [
            ("DASSIE_LOG_LEVEL", "verbose"),
            ("DASSIE_DEV_SECURITY_TOKEN", "short"),
            ("DASSIE_BOOTSTRAP_NODES", "{"),
            (
                "DASSIE_BOOTSTRAP_NODES",
                r#"[{"id":"dx","url":"https://a","publicKey":"QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"}]"#,
            ),
        ];
        for (variable, value) in bad {
            let result = settings(&[("HOME", "/h"), (variable, value)]);
            assert_eq!(result.err().map(|e| e.variable), Some(variable), "{value}");
        }
    }

    // NET-19: a bootstrap entry whose ID does not match its key is refused.
    #[test]
    fn bootstrap_ids_must_match_keys() {
        let swapped = r#"[{"id":"dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U","url":"https://test1.dassie.xyz","publicKey":"QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"}]"#;
        assert!(settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", swapped)]).is_err());
    }
}
