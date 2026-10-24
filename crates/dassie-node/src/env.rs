//! The daemon's environment, read once at start.
//!
//! The variable names and their precedence are the original's, minus those
//! that no longer mean anything: `NODE_ENV`, `DEBUG_HIDE_DATE`, the cache
//! and temporary directories, which nothing used (RT-4). The original evaluated them
//! lazily, so a bad value killed whichever actor first touched it and left
//! the daemon half-started (RT-24). Here every variable is checked before
//! anything starts, and a bad one stops the daemon with a message that names
//! it.
//!
//! The environment arrives as an [`Env`] snapshot rather than being read
//! through `std::env`, so tests can pass their own
//! (spec/decisions/tigerstyle.md).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use dassie_identity::{NodeId, PublicKey, base64url};
use dassie_node_core::config::Hostname;
use serde::Deserialize;

use crate::log::{LogFilter, LogLevel};

/// The public testnet's bootstrap nodes
/// (spec/decisions/bootstrap-network.md), one per region: boot1 in us-east1,
/// boot2 in europe-west3, boot3 in asia-northeast1. Each ID is checked
/// against its key at startup, like any entry (NET-19).
/// `DASSIE_BOOTSTRAP_NODES=[]` makes a node run alone.
pub const DEFAULT_BOOTSTRAP_NODES: &str = r#"[
  {"id":"dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk","url":"https://boot1.dassie.xyz","publicKey":"w25V-7IreZNDCPvwCrpiIv9Li5uwomzPSvqHot9ffLI"},
  {"id":"dE0GE90-9QCQI78S2_h5QaG81oo_cVKz1","url":"https://boot2.dassie.xyz","publicKey":"s9xgWff_UZ26HQwHT_aPd0q0SwyKPGXS74fd2xQjh14"},
  {"id":"dtTay2e6AV4J940nFJ6_WPkVL-TyrVKGW","url":"https://boot3.dassie.xyz","publicKey":"--P-gYMJ_ZZrFHa4GiNfVQ0x2jSu9_dr7z0mI_bcngY"}
]"#;

/// The relay a node keeps a connection to while it has no hostname, unless
/// `DASSIE_RELAY` names another: the front door of the node's default name,
/// `<label>.dassiepay.net`, which reaches the node only over that
/// connection (spec/decisions/default-relay.md,
/// spec/areas/node.md#a-chosen-relay). Its ID is checked against its key at
/// startup, like any entry. `DASSIE_RELAY=` (empty) keeps no relay.
pub const DEFAULT_RELAY: &str = r#"{"id":"ddTEO6qzWIorLvNGybj0uyK-mhlVJNa-q","url":"https://relay.dassiepay.net","publicKey":"DliIZHngMG7GTDqfHBSP_dfSTmn8ziA9OE37FnxC7eo"}"#;

/// Most bootstrap nodes accepted.
pub const MAX_BOOTSTRAP_NODES: usize = 64;

/// Most names the owner adds for Open Payments.
pub const MAX_PUBLIC_NAMES: usize = 16;

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

/// A node the daemon asks about the network
/// ([bootstrap nodes](../../../spec/areas/network.md#bootstrap-nodes)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapNode {
    /// Its node ID, which matches its key.
    pub id: NodeId,
    /// Its HTTPS URL.
    pub url: String,
    /// Its Ed25519 key.
    pub public_key: PublicKey,
}

/// The relay a node keeps a connection to
/// ([a chosen relay](../../../spec/areas/node.md#a-chosen-relay)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relay {
    /// Its ID, URL and key.
    pub node: BootstrapNode,
    /// Whether it is [`DEFAULT_RELAY`], as `DASSIE_RELAY` is unset: the
    /// node keeps it only while it has no hostname
    /// (spec/decisions/default-relay.md).
    pub is_default: bool,
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
    /// Where a development build of the wallet is written, to serve
    /// instead of the embedded one (`DASSIE_DEV_WALLET_DIR`).
    pub dev_wallet_dir: Option<PathBuf>,
    /// The network's bootstrap nodes.
    pub bootstrap_nodes: Vec<BootstrapNode>,
    /// The relay the node keeps a connection to, so that it can be reached
    /// through it (`DASSIE_RELAY`, an entry as in the bootstrap list):
    /// [`DEFAULT_RELAY`] when unset, none when empty.
    pub relay: Option<Relay>,
    /// An extra root the ACME client trusts, for test CAs such as Pebble.
    pub acme_ca_file: Option<PathBuf>,
    /// Extra roots the peer client trusts, beside the platform's: a dev or
    /// test CA (`DASSIE_PEER_CA_FILE`).
    pub peer_ca_file: Option<PathBuf>,
    /// Whether peers may be at private addresses, such as `localhost` in
    /// dev and tests (`DASSIE_PEER_ALLOW_PRIVATE=1`).
    pub peer_allow_private: bool,
    /// Whether the peer protocol runs on dev timers, polling every second
    /// (`DASSIE_DEV_TIMINGS=1`).
    pub dev_timings: bool,
    /// The names a node without a hostname answers Open Payments to, beside
    /// its default front door name (`DASSIE_PUBLIC_NAMES`, comma-separated;
    /// spec/areas/payments.md#the-nodes-names).
    pub public_names: Vec<Hostname>,
    /// The only nodes a periphery node asks to be its uplinks, as many as
    /// it then looks for (`DASSIE_UPLINKS`, node IDs, comma-separated;
    /// spec/areas/network.md#pinned-uplinks). Empty for any.
    pub uplinks: Vec<NodeId>,
}

impl Settings {
    /// Reads and checks every variable.
    pub fn from_env(env: &Env) -> Result<Self, EnvError> {
        Ok(Self {
            data_dir: data_dir(env)?,
            ipc_socket_path: env.path("DASSIE_IPC_SOCKET_PATH"),
            log_filter: log_filter(env)?,
            dev_wallet_dir: dev_wallet_dir(env)?,
            bootstrap_nodes: bootstrap_nodes(env)?,
            relay: relay(env)?,
            acme_ca_file: env.path("DASSIE_ACME_CA_FILE"),
            peer_ca_file: env.path("DASSIE_PEER_CA_FILE"),
            peer_allow_private: flag(env, "DASSIE_PEER_ALLOW_PRIVATE")?,
            dev_timings: flag(env, "DASSIE_DEV_TIMINGS")?,
            public_names: public_names(env)?,
            uplinks: uplinks(env)?,
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

/// What reaches standard output: `DASSIE_LOG_LEVEL`, and `DEBUG` for the
/// debug events shown.
///
/// # Errors
///
/// `DASSIE_LOG_LEVEL` names no level.
pub fn log_filter(env: &Env) -> Result<LogFilter, EnvError> {
    let level = match env.get("DASSIE_LOG_LEVEL")? {
        None => LogLevel::Info,
        Some(text) => LogLevel::parse(text).ok_or(EnvError {
            variable: "DASSIE_LOG_LEVEL",
            problem: format!("must be debug, info, warn, error or none, not {text:?}"),
        })?,
    };
    Ok(LogFilter::new(level, env.get("DEBUG")?.unwrap_or_default()))
}

/// A variable that is `1` or `0`, or absent for `0`.
fn flag(env: &Env, variable: &'static str) -> Result<bool, EnvError> {
    match env.get(variable)? {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(EnvError {
            variable,
            problem: "must be 1 or 0".into(),
        }),
    }
}

/// An absolute path, made canonical; the directory must exist.
fn dev_wallet_dir(env: &Env) -> Result<Option<PathBuf>, EnvError> {
    let Some(path) = env.path("DASSIE_DEV_WALLET_DIR") else {
        return Ok(None);
    };
    let error = |problem: &str| EnvError {
        variable: "DASSIE_DEV_WALLET_DIR",
        problem: problem.into(),
    };
    if !path.is_absolute() {
        return Err(error("must be an absolute path"));
    }
    match std::fs::canonicalize(&path) {
        Ok(path) if path.is_dir() => Ok(Some(path)),
        _ => Err(error("must be a directory that exists")),
    }
}

/// Hostnames, comma-separated, each once.
fn public_names(env: &Env) -> Result<Vec<Hostname>, EnvError> {
    let error = |problem: String| EnvError {
        variable: "DASSIE_PUBLIC_NAMES",
        problem,
    };
    let Some(text) = env.get("DASSIE_PUBLIC_NAMES")? else {
        return Ok(Vec::new());
    };
    let mut names: Vec<Hostname> = Vec::new();
    for entry in text
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if names.len() == MAX_PUBLIC_NAMES {
            return Err(error(format!("lists more than {MAX_PUBLIC_NAMES} names")));
        }
        let name =
            Hostname::parse(entry).map_err(|_| error(format!("has an invalid name {entry:?}")))?;
        if names.contains(&name) {
            return Err(error(format!("lists {name} more than once")));
        }
        names.push(name);
    }
    Ok(names)
}

/// Node IDs, comma-separated, each once, at most
/// [`dassie_peer::MAX_UPLINKS`] of them.
fn uplinks(env: &Env) -> Result<Vec<NodeId>, EnvError> {
    let error = |problem: String| EnvError {
        variable: "DASSIE_UPLINKS",
        problem,
    };
    let Some(text) = env.get("DASSIE_UPLINKS")? else {
        return Ok(Vec::new());
    };
    let mut uplinks: Vec<NodeId> = Vec::new();
    for entry in text
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if uplinks.len() == dassie_peer::MAX_UPLINKS {
            return Err(error(format!(
                "lists more than {} nodes",
                dassie_peer::MAX_UPLINKS
            )));
        }
        let id =
            NodeId::parse(entry).map_err(|_| error(format!("has an invalid node ID {entry:?}")))?;
        if uplinks.contains(&id) {
            return Err(error(format!("lists {id} more than once")));
        }
        uplinks.push(id);
    }
    Ok(uplinks)
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
    let nodes: Vec<BootstrapNode> = nodes
        .into_iter()
        .map(|node| {
            let checked = check_bootstrap_node(&node);
            checked.ok_or_else(|| error(format!("has an invalid entry for {:?}", node.id)))
        })
        .collect::<Result<_, _>>()?;
    // A node listed twice would vote twice for the majority.
    for (index, node) in nodes.iter().enumerate() {
        let repeated = nodes
            .iter()
            .skip(index + 1)
            .any(|other| other.id == node.id || other.url == node.url);
        if repeated {
            return Err(error(format!("lists {} more than once", node.id)));
        }
    }
    Ok(nodes)
}

/// `DASSIE_RELAY`: one entry in the bootstrap list's format; when unset,
/// [`DEFAULT_RELAY`]; when empty, none.
fn relay(env: &Env) -> Result<Option<Relay>, EnvError> {
    let (text, is_default) = match env.get("DASSIE_RELAY")? {
        None => (DEFAULT_RELAY, true),
        Some("") => return Ok(None),
        Some(text) => (text, false),
    };
    let error = |problem: String| EnvError {
        variable: "DASSIE_RELAY",
        problem,
    };
    let node: BootstrapNodeJson =
        serde_json::from_str(text).map_err(|e| error(format!("is not valid: {e}")))?;
    let node = check_bootstrap_node(&node)
        .ok_or_else(|| error(format!("is an invalid entry for {:?}", node.id)))?;
    Ok(Some(Relay { node, is_default }))
}

/// A bootstrap node's entry, if its URL is HTTPS, its key decodes and its
/// ID is the one the key gives.
fn check_bootstrap_node(node: &BootstrapNodeJson) -> Option<BootstrapNode> {
    let mut key = [0; 32];
    // `decode` asserts the output has room, so a longer key is refused
    // here rather than ending the node.
    if base64url::decoded_len(node.public_key.len()) != Some(key.len()) {
        return None;
    }
    base64url::decode(node.public_key.as_bytes(), &mut key).ok()?;
    if !dassie_peer::wire::is_valid_url(&node.url) {
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

    // spec/decisions/bootstrap-network.md: the default list names the
    // testnet's three bootstrap nodes, and each passes the same checks as a
    // configured entry.
    #[test]
    fn the_default_bootstrap_nodes_are_the_testnet() {
        let urls = settings(&[("HOME", "/h")]).map(|s| {
            s.bootstrap_nodes
                .iter()
                .map(|node| node.url.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            urls,
            Ok(["boot1", "boot2", "boot3"]
                .map(|host| format!("https://{host}.dassie.xyz"))
                .to_vec())
        );
    }

    #[test]
    fn an_empty_list_runs_alone() {
        let nodes = settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", "[]")])
            .map(|s| s.bootstrap_nodes.len());
        assert_eq!(nodes, Ok(0));
    }

    #[test]
    fn bootstrap_nodes_are_listed_once() {
        let one = r#"{"id":"dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW","url":"https://test1.dassie.xyz","publicKey":"QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"}"#;
        let twice = format!("[{one},{one}]");
        let once = format!("[{one}]");
        assert!(settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &twice)]).is_err());
        let nodes = settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &once)]);
        assert_eq!(nodes.map(|s| s.bootstrap_nodes.len()), Ok(1));
    }

    // RT-24: in the original these broke whichever actor read them first,
    // and the daemon ran on half-started.
    #[test]
    fn bad_values_are_errors_that_name_the_variable() {
        let bad = [
            ("DASSIE_LOG_LEVEL", "verbose"),
            ("DASSIE_DEV_WALLET_DIR", "relative/dist"),
            ("DASSIE_DEV_WALLET_DIR", "/no/such/directory"),
            ("DASSIE_PEER_ALLOW_PRIVATE", "yes"),
            ("DASSIE_PUBLIC_NAMES", "pay.example.com,not a name"),
            ("DASSIE_PUBLIC_NAMES", "pay.example.com,PAY.example.com"),
            ("DASSIE_PUBLIC_NAMES", "192.0.2.1"),
            ("DASSIE_UPLINKS", "boot1.dassie.xyz"),
            (
                "DASSIE_UPLINKS",
                "dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk,dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk",
            ),
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

    #[test]
    fn public_names_are_hostnames_in_their_order() {
        let names = |value: &str| {
            settings(&[("HOME", "/h"), ("DASSIE_PUBLIC_NAMES", value)]).map(|s| {
                s.public_names
                    .iter()
                    .map(|name| name.as_str().to_string())
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(names(""), Ok(vec![]));
        assert_eq!(
            names(" Pay.Example.com , other.example ,"),
            Ok(vec!["pay.example.com".into(), "other.example".into()])
        );
        let many = (0..=MAX_PUBLIC_NAMES)
            .map(|n| format!("n{n}.example"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(names(&many).is_err());
        let unset = settings(&[("HOME", "/h")]).map(|s| s.public_names.len());
        assert_eq!(unset, Ok(0));
    }

    // spec/areas/network.md#pinned-uplinks: node IDs, in their order, at
    // most as many as a node may be pinned to.
    #[test]
    fn uplinks_are_node_ids() {
        let uplinks = |value: &str| {
            settings(&[("HOME", "/h"), ("DASSIE_UPLINKS", value)]).map(|s| {
                s.uplinks
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
        };
        let (boot1, boot2) = (
            "dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk",
            "dE0GE90-9QCQI78S2_h5QaG81oo_cVKz1",
        );
        assert_eq!(uplinks(""), Ok(vec![]));
        assert_eq!(
            uplinks(&format!(" {boot2} , {boot1},")),
            Ok(vec![boot2.to_string(), boot1.to_string()])
        );
        // As many as a market maker's node may trade through, and no more.
        let many: Vec<String> = (0..=dassie_peer::MAX_UPLINKS)
            .map(|n| {
                let seed = [u8::try_from(n).unwrap(); 32];
                dassie_identity::NodeKey::from_bytes(&seed)
                    .node_id()
                    .to_string()
            })
            .collect();
        assert_eq!(many.len(), dassie_peer::MAX_UPLINKS + 1);
        assert!(uplinks(&many.join(",")).is_err());
        assert_eq!(
            uplinks(&many[1..].join(",")).map(|ids| ids.len()),
            Ok(dassie_peer::MAX_UPLINKS)
        );
        let twice = format!("{boot1},{boot2},{boot1}");
        assert!(uplinks(&twice).is_err());
        let unset = settings(&[("HOME", "/h")]).map(|s| s.uplinks.len());
        assert_eq!(unset, Ok(0));
    }

    // NET-19: a bootstrap entry whose ID does not match its key is refused.
    #[test]
    fn bootstrap_ids_must_match_keys() {
        let swapped = r#"[{"id":"dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U","url":"https://test1.dassie.xyz","publicKey":"QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"}]"#;
        assert!(settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", swapped)]).is_err());
    }

    // A key longer than 32 bytes once failed `decode`'s assertion on its
    // output buffer, which ended the node at its start.
    #[test]
    fn bootstrap_keys_of_another_length_are_refused() {
        for key in ["QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIMA", "QBe5", ""] {
            let nodes = format!(
                r#"[{{"id":"dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U","url":"https://test1.dassie.xyz","publicKey":"{key}"}}]"#
            );
            assert!(
                settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &nodes)]).is_err(),
                "{key}"
            );
        }
    }

    // The chosen relay is one entry in the bootstrap list's format, checked
    // the same way.
    #[test]
    fn a_relay_is_one_checked_entry() {
        let relay = |value: &str| settings(&[("HOME", "/h"), ("DASSIE_RELAY", value)]);
        let boot1 = r#"{"id":"dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk","url":"https://boot1.dassie.xyz","publicKey":"w25V-7IreZNDCPvwCrpiIv9Li5uwomzPSvqHot9ffLI"}"#;
        let chosen = relay(boot1).unwrap().relay.unwrap();
        assert_eq!(chosen.node.url, "https://boot1.dassie.xyz");
        assert_eq!(chosen.node.public_key.node_id(), chosen.node.id);
        // One the owner named is kept, hostname or not.
        assert!(!chosen.is_default);
        let swapped = r#"{"id":"dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U","url":"https://boot1.dassie.xyz","publicKey":"w25V-7IreZNDCPvwCrpiIv9Li5uwomzPSvqHot9ffLI"}"#;
        assert!(relay(swapped).is_err());
        assert!(relay(&format!("[{boot1}]")).is_err());
        let http = boot1.replace("https://", "http://");
        assert!(relay(&http).is_err());
    }

    // spec/decisions/default-relay.md: unset, the relay is the front door of
    // the default name, checked as any entry and kept only without a
    // hostname; empty, there is none.
    #[test]
    fn the_default_relay_is_the_default_names_front_door() {
        let unset = settings(&[("HOME", "/h")]).unwrap().relay.unwrap();
        assert!(unset.is_default);
        assert_eq!(
            unset.node.url,
            format!(
                "https://relay.{}",
                crate::open_payments::DEFAULT_FRONT_DOOR_DOMAIN
            )
        );
        assert_eq!(unset.node.public_key.node_id(), unset.node.id);
        let empty = settings(&[("HOME", "/h"), ("DASSIE_RELAY", "")]);
        assert_eq!(empty.map(|s| s.relay), Ok(None));
    }
}
