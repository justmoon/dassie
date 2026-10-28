//! The daemon's environment, read once at start.
//!
//! The variable names and their precedence are the original's, minus those
//! that no longer mean anything: `NODE_ENV`, `DEBUG_HIDE_DATE`, the cache
//! and temporary directories, which nothing used. The original evaluated them
//! lazily, so a bad value killed whichever actor first touched it and left
//! the daemon half-started. Here every variable is checked before
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

use dassie_identity::{NodeId, NodeIdentity, PublicKey, SlhDsaPublicKey};
use dassie_node_core::config::{Hostname, LiveRealm};
use dassie_peer::wire::Endpoint;
use serde::Deserialize;

use crate::log::LogFilter;
use crate::quic::proxy::{NoProxy, Proxy};

/// The public testnet's bootstrap nodes
/// (spec/decisions/bootstrap-network.md), in one region, us-east1, so that
/// they can validate the testnet's settlement hub: boot1 and boot4 in zone
/// b, boot2 in c, boot3 in d. Each entry names a node's ID, URL and its two
/// keys, Ed25519 and SLH-DSA, and each ID is checked against its keys at
/// startup, like any entry. `DASSIE_BOOTSTRAP_NODES=[]` makes a node run
/// alone.
pub const DEFAULT_BOOTSTRAP_NODES: &str = r#"[
  {"id":"32f6pclvm5p5xxdeqigvgxpjezayxacvmnnwgcy","url":"https://boot1.dassie.xyz","publicKey":"2DJ7XMcmzgUu-Ec50PSdPvNHL5_4PKo6WmYrP4nMHEU","slhDsaPublicKey":"4nvvGnn31qeJEYBdzzulzaDcIpFtdFq9HeNhA6fJNeg"},
  {"id":"wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby","url":"https://boot2.dassie.xyz","publicKey":"FWAmSiJvL2c0c_6y4qdcWL3BVUF0j1AccrS7IrS4eRw","slhDsaPublicKey":"Usj-SMIJ2xBwIPIe5-TcOMH0sElKvDl344ut413AJj4"},
  {"id":"2mbtvg34lgihhdssw7v3ecwiogzs5l4jondqxjq","url":"https://boot3.dassie.xyz","publicKey":"MENy-bobL2KCbx2c12FO2aoQvaeWctWtPR38Gtz6qYY","slhDsaPublicKey":"JSGuAoeDSnid0EjtM2bbdt5ltY5ihJMj9ALizTUTjjk"},
  {"id":"cxctokwbdeczn2ganlberxwwghzyp6xs2coj2qq","url":"https://boot4.dassie.xyz","publicKey":"SwEEVe2I4YSvM-814iProgG_PauuBMWnk49MjW2DMXQ","slhDsaPublicKey":"yftGkX5GBqNR3lJJ6kGcDlbKtgzNDK5F2N9zanQVJMQ"}
]"#;

/// The relay a node keeps a connection to while it has no hostname, unless
/// `DASSIE_RELAY` names another: the front door of the node's default name,
/// `<node ID>.dassiepay.net`, which reaches the node only over that
/// connection (spec/decisions/default-relay.md,
/// spec/areas/node.md#a-chosen-relay). Its ID is checked against its keys
/// at startup, like any entry. `DASSIE_RELAY=` (empty) keeps no relay.
///
/// None until the testnet's reset: the front door's ID now commits to its
/// SLH-DSA key, which derives from its key file, so its entry is the one it
/// logs once it runs a release that makes it (`relay_entry` in
/// `front_door/key.rs`). Until then a node keeps no relay by default.
pub const DEFAULT_RELAY: Option<&str> = None;

/// Most bootstrap nodes accepted: each a registrar whose name list every
/// node reads (spec/areas/network.md#holders).
pub const MAX_BOOTSTRAP_NODES: usize = 64;

const _: () = assert!(MAX_BOOTSTRAP_NODES <= dassie_peer::names::MAX_REGISTRARS);

/// Most names the owner adds for Open Payments.
pub const MAX_PUBLIC_NAMES: usize = 16;

/// Most node IDs `DASSIE_UPLINKS` names: the most uplinks a node pins.
pub const MAX_UPLINKS: usize = dassie_peer::MAX_UPLINKS;

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
    /// Its node ID, which matches its keys.
    pub id: NodeId,
    /// Where it takes QUIC, read from its entry's URL.
    pub endpoint: Endpoint,
    /// Its keys: its Ed25519 key and its SLH-DSA key.
    pub identity: NodeIdentity,
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
    /// Extra roots the node's outgoing client trusts, beside the
    /// platform's, for peers, relays, front doors and every other server
    /// it reaches: a dev or test CA (`DASSIE_EXTRA_CA_FILE`).
    pub extra_ca_file: Option<PathBuf>,
    /// Whether peers may be at private addresses, such as `localhost` in
    /// dev and tests (`DASSIE_PEER_ALLOW_PRIVATE=1`).
    pub peer_allow_private: bool,
    /// Whether the peer protocol and the peg guard run on dev timers,
    /// polling and reading pools every second (`DASSIE_DEV_TIMINGS=1`).
    pub dev_timings: bool,
    /// Whether the node may run in the live realm, for dev and the
    /// end-to-end tests against a fake ledger (`DASSIE_DEV_LIVE_REALM=1`):
    /// only a build with debug assertions takes the switch
    /// (spec/decisions/mainnet-schemes.md).
    pub live_realm: LiveRealm,
    /// The names a node without a hostname answers Open Payments to, beside
    /// its default front door name (`DASSIE_PUBLIC_NAMES`, comma-separated;
    /// spec/areas/payments.md#the-nodes-names).
    pub public_names: Vec<Hostname>,
    /// The only nodes a periphery node asks to be its uplinks, as many as
    /// it then looks for (`DASSIE_UPLINKS`, node IDs, comma-separated;
    /// spec/areas/network.md#pinned-uplinks). Empty for any.
    pub uplinks: Vec<NodeId>,
    /// The settlement hubs the node validates, and the sponsors it
    /// validates for (`DASSIE_VALIDATOR_CONFIG`, a file;
    /// spec/areas/hubs.md#the-validator-in-the-node). None when unset:
    /// validating is something an owner turns on.
    pub validator: Option<crate::hub::config::ValidatorConfig>,
    /// The HTTP proxy the node's tunnels dial through, and the hosts they
    /// dial directly (`https_proxy` and `no_proxy`, or `HTTPS_PROXY` and
    /// `NO_PROXY`; spec/areas/peer-protocol.md#quic-over-websocket). None
    /// when unset, or set to a proxy the node cannot use.
    pub tunnel_proxy: Option<Proxy>,
    /// What the node could not use of those variables, which are set for
    /// every program and so never stop it: the daemon warns of each at
    /// start. At most three.
    pub tunnel_proxy_problems: Vec<EnvError>,
}

impl Settings {
    /// Reads and checks every variable.
    pub fn from_env(env: &Env) -> Result<Self, EnvError> {
        let (tunnel_proxy, tunnel_proxy_problems) = tunnel_proxy(env);
        let live_realm = live_realm(env, cfg!(debug_assertions))?;
        Ok(Self {
            data_dir: data_dir(env)?,
            ipc_socket_path: env.path("DASSIE_IPC_SOCKET_PATH"),
            log_filter: log_filter(env)?,
            dev_wallet_dir: dev_wallet_dir(env)?,
            bootstrap_nodes: bootstrap_nodes(env)?,
            relay: relay(env)?,
            acme_ca_file: env.path("DASSIE_ACME_CA_FILE"),
            extra_ca_file: env.path("DASSIE_EXTRA_CA_FILE"),
            peer_allow_private: flag(env, "DASSIE_PEER_ALLOW_PRIVATE")?,
            dev_timings: flag(env, "DASSIE_DEV_TIMINGS")?,
            live_realm,
            public_names: public_names(env)?,
            uplinks: uplinks(env)?,
            validator: validator(env, live_realm)?,
            tunnel_proxy,
            tunnel_proxy_problems,
        })
    }
}

/// The validator role's file, read and checked now, at most
/// [`MAX_CONFIG_BYTES`](crate::hub::config::MAX_CONFIG_BYTES).
fn validator(
    env: &Env,
    live: LiveRealm,
) -> Result<Option<crate::hub::config::ValidatorConfig>, EnvError> {
    use std::io::Read as _;
    let Some(path) = env.path("DASSIE_VALIDATOR_CONFIG") else {
        return Ok(None);
    };
    let error = |problem: String| EnvError {
        variable: "DASSIE_VALIDATOR_CONFIG",
        problem,
    };
    let file = std::fs::File::open(&path)
        .map_err(|e| error(format!("names a file that cannot be read: {e}")))?;
    let limit = crate::hub::config::MAX_CONFIG_BYTES;
    let mut text = String::new();
    file.take(limit.saturating_add(1))
        .read_to_string(&mut text)
        .map_err(|e| error(format!("names a file that cannot be read as text: {e}")))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > limit {
        return Err(error(format!("names a file larger than {limit} bytes")));
    }
    crate::hub::config::parse(&text, live)
        .map(Some)
        .map_err(|problem| error(format!("names a file that is wrong: {problem}")))
}

/// The proxy for the tunnels the node dials, as other programs read it:
/// `https_proxy`, else `HTTPS_PROXY`, bypassed for the hosts `no_proxy`,
/// else `NO_PROXY`, lists. An empty variable is unset. `NO_PROXY` is read
/// only with a proxy, since it means nothing without one.
///
/// These variables are the network's, set for every program, so what the
/// node cannot use never stops it: a proxy it cannot use is ignored, and
/// entries of `NO_PROXY` it cannot read are skipped, each with a problem
/// to warn of at start, at most three. A variable that is not UTF-8 counts
/// as unset, with a problem too.
fn tunnel_proxy(env: &Env) -> (Option<Proxy>, Vec<EnvError>) {
    let mut problems = Vec::with_capacity(3);
    let Some((variable, url)) = first_set(env, ["https_proxy", "HTTPS_PROXY"], &mut problems)
    else {
        return (None, problems);
    };
    let mut bypass_problems = Vec::with_capacity(2);
    let bypass = match first_set(env, ["no_proxy", "NO_PROXY"], &mut bypass_problems) {
        None => NoProxy::default(),
        Some((variable, text)) => {
            let (bypass, problem) = NoProxy::parse(text);
            bypass_problems.extend(problem.map(|problem| EnvError {
                variable,
                problem: format!("{problem}: tunnels to those hosts go through the proxy"),
            }));
            bypass
        }
    };
    match Proxy::parse(url, bypass) {
        Ok(proxy) => {
            problems.extend(bypass_problems);
            assert!(problems.len() <= 3);
            (Some(proxy), problems)
        }
        // With no proxy, `NO_PROXY` means nothing.
        Err(problem) => {
            problems.push(EnvError {
                variable,
                problem: format!("{problem}: the node's tunnels go without a proxy"),
            });
            (None, problems)
        }
    }
}

/// The first of `variables` set to more than nothing, and its value; one
/// that is not UTF-8 is skipped, with its problem added to `problems`.
fn first_set<'a>(
    env: &'a Env,
    variables: [&'static str; 2],
    problems: &mut Vec<EnvError>,
) -> Option<(&'static str, &'a str)> {
    for variable in variables {
        match env.get(variable) {
            Ok(Some(value)) if !value.is_empty() => return Some((variable, value)),
            Ok(_) => {}
            Err(EnvError { variable, problem }) => problems.push(EnvError {
                variable,
                problem: format!("{problem}: the node takes it as unset"),
            }),
        }
    }
    None
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

/// What reaches standard output: `DASSIE_LOG`, `info` if unset.
///
/// # Errors
///
/// `DASSIE_LOG` holds a directive that is not one.
pub fn log_filter(env: &Env) -> Result<LogFilter, EnvError> {
    LogFilter::parse(env.get("DASSIE_LOG")?.unwrap_or_default()).map_err(|problem| EnvError {
        variable: "DASSIE_LOG",
        problem,
    })
}

/// A variable that is `1` or `0`, or absent for `0`.
///
/// # Errors
///
/// It is something else.
pub fn flag(env: &Env, variable: &'static str) -> Result<bool, EnvError> {
    match env.get(variable)? {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(EnvError {
            variable,
            problem: "must be 1 or 0".into(),
        }),
    }
}

/// The live realm's dev switch, `DASSIE_DEV_LIVE_REALM`, in a build with
/// debug assertions or without: a build without them refuses to start
/// with it set, so no release binary is turned to real money by a variable
/// (spec/decisions/mainnet-schemes.md).
fn live_realm(env: &Env, debug_assertions: bool) -> Result<LiveRealm, EnvError> {
    const VARIABLE: &str = "DASSIE_DEV_LIVE_REALM";
    match (flag(env, VARIABLE)?, debug_assertions) {
        (false, _) => Ok(LiveRealm::Refused),
        (true, true) => Ok(LiveRealm::DevSwitch),
        (true, false) => Err(EnvError {
            variable: VARIABLE,
            problem: "is a development switch, which a release build refuses: \
                      the live realm is not supported yet"
                .into(),
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
    slh_dsa_public_key: String,
}

/// `DASSIE_BOOTSTRAP_NODES`: a JSON list of entries, each checked; when
/// unset, [`DEFAULT_BOOTSTRAP_NODES`]. A front door reads it as a node
/// does, for the registrars whose name lists it reads
/// (spec/areas/http.md#front-doors).
///
/// # Errors
///
/// The list is not valid, is too long, or names a node twice.
pub fn bootstrap_nodes(env: &Env) -> Result<Vec<BootstrapNode>, EnvError> {
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
            .any(|other| other.id == node.id || other.endpoint == node.endpoint);
        if repeated {
            return Err(error(format!("lists {} more than once", node.id)));
        }
    }
    Ok(nodes)
}

/// `DASSIE_RELAY`: one entry in the bootstrap list's format; when unset,
/// [`DEFAULT_RELAY`], if there is one; when empty, none.
fn relay(env: &Env) -> Result<Option<Relay>, EnvError> {
    let (text, is_default) = match env.get("DASSIE_RELAY")? {
        None => match DEFAULT_RELAY {
            Some(text) => (text, true),
            None => return Ok(None),
        },
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

/// A bootstrap node's entry, if its URL is HTTPS, its keys decode and its
/// ID is the one the two keys give.
fn check_bootstrap_node(node: &BootstrapNodeJson) -> Option<BootstrapNode> {
    let endpoint = Endpoint::of_url(&node.url)?;
    let public_key = PublicKey::from_bytes(&key_bytes(&node.public_key)?).ok()?;
    let slh_dsa = SlhDsaPublicKey::from_bytes(key_bytes(&node.slh_dsa_public_key)?);
    let identity = NodeIdentity::new(public_key, slh_dsa);
    let id = NodeId::parse(&node.id).ok()?;
    (identity.node_id() == id).then_some(BootstrapNode {
        id,
        endpoint,
        identity,
    })
}

/// The 32 bytes of a key in base64url. A key must decode to exactly 32
/// bytes: a shorter one would leave the rest zero.
fn key_bytes(text: &str) -> Option<[u8; 32]> {
    let mut key = [0; 32];
    if dassie_base64url::decoded_len(text.len()) != Some(key.len()) {
        return None;
    }
    dassie_base64url::decode(text.as_bytes(), &mut key).ok()?;
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dassie_identity::IdentityKey;

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

    /// The entry of the node of seed `seed` at `url`, as a front door logs
    /// its own.
    fn entry(seed: u8, url: &str) -> String {
        let key = IdentityKey::from_bytes(&[seed; 32]);
        crate::front_door::key::relay_entry(&key, url)
    }

    /// The same, but whose `field` holds `value`.
    fn entry_with(seed: u8, url: &str, field: &str, value: &str) -> String {
        let mut entry: serde_json::Value = serde_json::from_str(&entry(seed, url)).unwrap();
        entry[field] = value.into();
        entry.to_string()
    }

    // spec/decisions/bootstrap-network.md: the default list names boot1–4,
    // each passing the checks of a configured entry, its ID among them.
    #[test]
    fn the_default_bootstrap_nodes_are_the_testnet() {
        let nodes = settings(&[("HOME", "/h")]).map(|s| {
            s.bootstrap_nodes
                .iter()
                .map(|node| (node.id.as_str().to_owned(), node.endpoint.url()))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            nodes,
            Ok([
                ("32f6pclvm5p5xxdeqigvgxpjezayxacvmnnwgcy", "boot1"),
                ("wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby", "boot2"),
                ("2mbtvg34lgihhdssw7v3ecwiogzs5l4jondqxjq", "boot3"),
                ("cxctokwbdeczn2ganlberxwwghzyp6xs2coj2qq", "boot4"),
            ]
            .map(|(id, host)| (id.to_owned(), format!("https://{host}.dassie.xyz")))
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
        let one = entry(1, "https://test1.dassie.xyz");
        let twice = format!("[{one},{one}]");
        let once = format!("[{one}]");
        assert!(settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &twice)]).is_err());
        let nodes = settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &once)]);
        assert_eq!(nodes.map(|s| s.bootstrap_nodes.len()), Ok(1));
    }

    // A bad value stops the start before anything runs, and names the
    // variable.
    #[test]
    fn bad_values_are_errors_that_name_the_variable() {
        let bad = [
            ("DASSIE_LOG", "verbose"),
            ("DASSIE_LOG", "dassie::peer=loud"),
            ("DASSIE_DEV_WALLET_DIR", "relative/dist"),
            ("DASSIE_DEV_WALLET_DIR", "/no/such/directory"),
            ("DASSIE_PEER_ALLOW_PRIVATE", "yes"),
            ("DASSIE_DEV_LIVE_REALM", "live"),
            ("DASSIE_PUBLIC_NAMES", "pay.example.com,not a name"),
            ("DASSIE_PUBLIC_NAMES", "pay.example.com,PAY.example.com"),
            ("DASSIE_PUBLIC_NAMES", "192.0.2.1"),
            ("DASSIE_UPLINKS", "boot1.dassie.xyz"),
            (
                "DASSIE_UPLINKS",
                "br5l3hqpjhlt5znzb24hrxqme77d7q7crlqfhza,br5l3hqpjhlt5znzb24hrxqme77d7q7crlqfhza",
            ),
            ("DASSIE_BOOTSTRAP_NODES", "{"),
            (
                "DASSIE_BOOTSTRAP_NODES",
                &format!("[{}]", entry_with(1, "https://a", "id", "dx")),
            ),
            ("DASSIE_VALIDATOR_CONFIG", "/no/such/file"),
        ];
        for (variable, value) in bad {
            let result = settings(&[("HOME", "/h"), (variable, value)]);
            assert_eq!(result.err().map(|e| e.variable), Some(variable), "{value}");
        }
    }

    // spec/decisions/mainnet-schemes.md: the live realm's dev switch is
    // honoured only by a build with debug assertions, as tests are; a
    // release build refuses to start with it set, naming it, and runs as
    // ever without it.
    #[test]
    fn a_release_build_refuses_the_live_realms_dev_switch() {
        let on = Env::from_pairs([("DASSIE_DEV_LIVE_REALM", "1")]);
        let off = Env::from_pairs([("DASSIE_DEV_LIVE_REALM", "0")]);
        assert_eq!(live_realm(&on, true), Ok(LiveRealm::DevSwitch));
        assert_eq!(
            live_realm(&on, false).map_err(|error| error.variable),
            Err("DASSIE_DEV_LIVE_REALM")
        );
        for debug_assertions in [true, false] {
            assert_eq!(live_realm(&off, debug_assertions), Ok(LiveRealm::Refused));
            assert_eq!(
                live_realm(&Env::default(), debug_assertions),
                Ok(LiveRealm::Refused)
            );
        }
        let switched = settings(&[("HOME", "/h"), ("DASSIE_DEV_LIVE_REALM", "1")]);
        assert_eq!(switched.map(|s| s.live_realm), Ok(LiveRealm::DevSwitch));
    }

    // spec/areas/hubs.md#the-validator-in-the-node: validating is opt-in,
    // and a file that is wrong stops the daemon, naming the variable.
    #[test]
    fn the_validator_config_is_read_from_its_file() {
        let none = settings(&[("HOME", "/h")]).map(|s| s.validator.is_none());
        assert_eq!(none, Ok(true));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("validator.json");
        std::fs::write(&path, r#"{"sponsors": [], "hubs": []}"#).unwrap();
        let path_text = path.to_str().unwrap();
        let config = settings(&[("HOME", "/h"), ("DASSIE_VALIDATOR_CONFIG", path_text)]);
        assert_eq!(
            config.map(|s| s.validator.map(|v| v.hubs.len())),
            Ok(Some(0))
        );
        std::fs::write(&path, r#"{"sponsors": ["not an ID"], "hubs": []}"#).unwrap();
        let wrong = settings(&[("HOME", "/h"), ("DASSIE_VALIDATOR_CONFIG", path_text)]);
        let error = wrong.unwrap_err();
        assert_eq!(error.variable, "DASSIE_VALIDATOR_CONFIG");
        assert!(
            error.problem.contains("invalid sponsor"),
            "{}",
            error.problem
        );
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
            "br5l3hqpjhlt5znzb24hrxqme77d7q7crlqfhza",
            "cnayj52pxvacichpys3p4hsqnbxtliup3rkkz5i",
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
                dassie_identity::IdentityKey::from_bytes(&seed)
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

    // A bootstrap entry whose ID does not match its keys is refused: the ID
    // of another node, or of its Ed25519 key with another SLH-DSA key.
    #[test]
    fn bootstrap_ids_must_match_keys() {
        let url = "https://test1.dassie.xyz";
        let other = IdentityKey::from_bytes(&[2; 32]).node_id();
        let swapped = entry_with(1, url, "id", other.as_str());
        let mut slh_dsa = [0u8; 43];
        let length = dassie_base64url::encode(&[9; 32], &mut slh_dsa);
        let slh_dsa = std::str::from_utf8(&slh_dsa[..length]).unwrap();
        let other_slh_dsa = entry_with(1, url, "slhDsaPublicKey", slh_dsa);
        let mut without: serde_json::Value = serde_json::from_str(&entry(1, url)).unwrap();
        without.as_object_mut().unwrap().remove("slhDsaPublicKey");
        for nodes in [swapped, other_slh_dsa, without.to_string()] {
            let nodes = format!("[{nodes}]");
            assert!(settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &nodes)]).is_err());
        }
    }

    // A key longer than 32 bytes once failed `decode`'s assertion on its
    // output buffer, which ended the node at its start.
    #[test]
    fn bootstrap_keys_of_another_length_are_refused() {
        for key in ["QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIMA", "QBe5", ""] {
            for field in ["publicKey", "slhDsaPublicKey"] {
                let nodes = format!(
                    "[{}]",
                    entry_with(1, "https://test1.dassie.xyz", field, key)
                );
                assert!(
                    settings(&[("HOME", "/h"), ("DASSIE_BOOTSTRAP_NODES", &nodes)]).is_err(),
                    "{field} {key}"
                );
            }
        }
    }

    // The chosen relay is one entry in the bootstrap list's format, checked
    // the same way.
    #[test]
    fn a_relay_is_one_checked_entry() {
        let relay = |value: &str| settings(&[("HOME", "/h"), ("DASSIE_RELAY", value)]);
        let boot1 = entry(1, "https://boot1.dassie.xyz");
        let chosen = relay(&boot1).unwrap().relay.unwrap();
        assert_eq!(chosen.node.endpoint.url(), "https://boot1.dassie.xyz");
        assert_eq!(chosen.node.identity.node_id(), chosen.node.id);
        // One the owner named is kept, hostname or not.
        assert!(!chosen.is_default);
        let other = IdentityKey::from_bytes(&[2; 32]).node_id();
        let swapped = entry_with(1, "https://boot1.dassie.xyz", "id", other.as_str());
        assert!(relay(&swapped).is_err());
        assert!(relay(&format!("[{boot1}]")).is_err());
        let http = boot1.replace("https://", "http://");
        assert!(relay(&http).is_err());
    }

    // spec/decisions/default-relay.md: there is no default relay until the
    // testnet's reset gives the front door of the default name an identity
    // whose ID commits to its SLH-DSA key, so unset, as empty, keeps none.
    // The reset builds the front door's entry in, and this test then checks
    // that unset names it, checked as any entry and kept only without a
    // hostname.
    #[test]
    fn the_default_relay_is_none_until_the_reset() {
        assert_eq!(DEFAULT_RELAY, None);
        let unset = settings(&[("HOME", "/h")]);
        assert_eq!(unset.map(|s| s.relay), Ok(None));
        let empty = settings(&[("HOME", "/h"), ("DASSIE_RELAY", "")]);
        assert_eq!(empty.map(|s| s.relay), Ok(None));
    }
    // spec/areas/peer-protocol.md#quic-over-websocket: the proxy for
    // tunnels is read as other programs read it, lowercase first, and
    // `NO_PROXY` only with a proxy.
    #[test]
    fn the_tunnels_proxy_is_read_as_other_programs_read_it() {
        let proxy = |pairs: &[(&str, &str)]| {
            let mut all = vec![("HOME", "/h")];
            all.extend_from_slice(pairs);
            settings(&all).map(|s| s.tunnel_proxy.map(|proxy| proxy.to_string()))
        };
        assert_eq!(proxy(&[]), Ok(None));
        assert_eq!(proxy(&[("HTTPS_PROXY", "")]), Ok(None));
        assert_eq!(
            proxy(&[("HTTPS_PROXY", "http://upper:1")]),
            Ok(Some("upper:1".to_owned()))
        );
        assert_eq!(
            proxy(&[
                ("HTTPS_PROXY", "http://upper:1"),
                ("https_proxy", "http://lower:2")
            ]),
            Ok(Some("lower:2".to_owned()))
        );
        // An empty lowercase one leaves the uppercase one.
        assert_eq!(
            proxy(&[("HTTPS_PROXY", "http://upper:1"), ("https_proxy", "")]),
            Ok(Some("upper:1".to_owned()))
        );
        let settings = settings(&[
            ("HOME", "/h"),
            ("HTTPS_PROXY", "http://p:1"),
            ("NO_PROXY", "near.example"),
        ])
        .unwrap();
        assert_eq!(settings.tunnel_proxy_problems, []);
        let proxy = settings.tunnel_proxy.unwrap();
        assert!(!proxy.applies_to("near.example"));
        assert!(proxy.applies_to("far.example"));
    }

    // spec/areas/node.md#startup: the proxy variables are set for every
    // program, so what the node cannot use of them never stops it: it
    // ignores a proxy it cannot use, and skips what it cannot read of
    // `NO_PROXY`, naming the variable for a warning.
    #[test]
    fn a_proxy_the_node_cannot_use_never_stops_it() {
        let read = |pairs: &[(&str, &str)]| {
            let mut all = vec![("HOME", "/h")];
            all.extend_from_slice(pairs);
            let settings = settings(&all).unwrap();
            let problems = settings.tunnel_proxy_problems.iter();
            let variables: Vec<_> = problems.map(|problem| problem.variable).collect();
            (
                settings.tunnel_proxy.map(|proxy| proxy.to_string()),
                variables,
            )
        };
        for url in [
            "socks5://127.0.0.1:1080",
            "https://proxy.example:443",
            "http://proxy.example:port",
        ] {
            let ignored = read(&[("HTTPS_PROXY", url), ("NO_PROXY", "host:8080")]);
            assert_eq!(ignored, (None, vec!["HTTPS_PROXY"]), "{url}");
        }
        // `NO_PROXY` is read only with a proxy; what it cannot read is
        // skipped, and the rest kept.
        assert_eq!(read(&[("NO_PROXY", "host:8080")]), (None, vec![]));
        let skipped = read(&[
            ("HTTPS_PROXY", "http://p:1"),
            ("no_proxy", "localhost:3000,10.*,near.example"),
        ]);
        assert_eq!(skipped, (Some("p:1".to_owned()), vec!["no_proxy"]));
        let many = vec!["a.example"; crate::quic::proxy::MAX_NO_PROXY_ENTRIES + 1].join(",");
        let long = read(&[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", &many)]);
        assert_eq!(long, (Some("p:1".to_owned()), vec!["NO_PROXY"]));
    }

    // A proxy variable that is not UTF-8 counts as unset, with a warning,
    // rather than stopping the node.
    #[cfg(unix)]
    #[test]
    fn a_proxy_variable_that_is_not_utf8_never_stops_the_node() {
        use std::os::unix::ffi::OsStringExt as _;
        let mut env = Env::from_pairs([("HOME", "/h"), ("HTTPS_PROXY", "http://p:1")]);
        env.0
            .insert("https_proxy".into(), OsString::from_vec(vec![0xff]));
        let settings = Settings::from_env(&env).unwrap();
        assert_eq!(
            settings.tunnel_proxy.map(|p| p.to_string()),
            Some("p:1".into())
        );
        let problems = settings.tunnel_proxy_problems.iter();
        let variables: Vec<_> = problems.map(|problem| problem.variable).collect();
        assert_eq!(variables, ["https_proxy"]);
    }
}
