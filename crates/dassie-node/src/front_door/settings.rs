//! A front door's settings, from its environment; `dassie front-door`
//! lets flags override them.

use std::net::SocketAddr;
use std::path::PathBuf;

use dassie_node_core::config::Hostname;

use crate::env::{Env, EnvError, log_filter};
use crate::log::LogFilter;
use crate::open_payments::DEFAULT_FRONT_DOOR_DOMAIN;

/// Where both listeners are by default: every address, on 443, TCP for
/// HTTPS and UDP for QUIC.
const DEFAULT_LISTEN: &str = "[::]:443";

/// The key file's name in systemd's state directory, when no path is given.
const KEY_FILE_NAME: &str = "front-door.key";

/// What a front door runs with.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The domain it serves nodes under, as `<label>.<domain>`
    /// (`DASSIE_FRONT_DOOR_DOMAIN`).
    pub domain: Hostname,
    /// The HTTPS listener (`DASSIE_FRONT_DOOR_HTTPS_LISTEN`).
    pub https_listen: SocketAddr,
    /// The QUIC endpoint nodes connect to (`DASSIE_FRONT_DOOR_QUIC_LISTEN`).
    pub quic_listen: SocketAddr,
    /// The certificate chain for `*.<domain>`, PEM, leaf first
    /// (`DASSIE_FRONT_DOOR_CERTIFICATE`).
    pub certificate: Option<PathBuf>,
    /// The certificate's key, PEM (`DASSIE_FRONT_DOOR_CERTIFICATE_KEY`).
    pub certificate_key: Option<PathBuf>,
    /// The file with its own Ed25519 key, a 32-byte seed or PKCS#8
    /// (`DASSIE_FRONT_DOOR_KEY_FILE`; by default in systemd's state
    /// directory).
    pub key_file: Option<PathBuf>,
    /// The URL nodes dial its QUIC endpoint at, for the `DASSIE_RELAY`
    /// entry it prints (`DASSIE_FRONT_DOOR_RELAY_URL`; by default
    /// `https://relay.<domain>`).
    pub relay_url: Option<String>,
    /// What reaches standard output.
    pub log_filter: LogFilter,
}

impl Settings {
    /// Reads and checks every variable.
    ///
    /// # Errors
    ///
    /// A variable has a value the front door cannot use.
    pub fn from_env(env: &Env) -> Result<Self, EnvError> {
        let domain = env
            .get("DASSIE_FRONT_DOOR_DOMAIN")?
            .unwrap_or(DEFAULT_FRONT_DOOR_DOMAIN);
        let key_file = env.path("DASSIE_FRONT_DOOR_KEY_FILE").or_else(|| {
            let states = env.get("STATE_DIRECTORY").ok().flatten()?;
            let first = states.split(':').find(|entry| !entry.is_empty())?;
            Some(PathBuf::from(first).join(KEY_FILE_NAME))
        });
        Ok(Self {
            domain: parse_domain(domain).map_err(|problem| EnvError {
                variable: "DASSIE_FRONT_DOOR_DOMAIN",
                problem,
            })?,
            https_listen: listen(env, "DASSIE_FRONT_DOOR_HTTPS_LISTEN")?,
            quic_listen: listen(env, "DASSIE_FRONT_DOOR_QUIC_LISTEN")?,
            certificate: env.path("DASSIE_FRONT_DOOR_CERTIFICATE"),
            certificate_key: env.path("DASSIE_FRONT_DOOR_CERTIFICATE_KEY"),
            key_file,
            relay_url: match env.get("DASSIE_FRONT_DOOR_RELAY_URL")? {
                None | Some("") => None,
                Some(url) => Some(parse_relay_url(url).map_err(|problem| EnvError {
                    variable: "DASSIE_FRONT_DOOR_RELAY_URL",
                    problem,
                })?),
            },
            log_filter: log_filter(env)?,
        })
    }

    /// The URL nodes dial the QUIC endpoint at: the one given, or
    /// `https://relay.<domain>`.
    #[must_use]
    pub fn relay_url(&self) -> String {
        self.relay_url
            .clone()
            .unwrap_or_else(|| format!("https://relay.{}", self.domain))
    }
}

/// A domain to serve nodes under: a hostname of at least two labels, so
/// that the front door never serves a top-level domain's names.
///
/// # Errors
///
/// What is wrong with it.
pub fn parse_domain(text: &str) -> Result<Hostname, String> {
    let domain = Hostname::parse(text).map_err(|_| format!("{text:?} is not a hostname"))?;
    if domain.as_str().contains('.') {
        Ok(domain)
    } else {
        Err(format!("{text:?} has a single label"))
    }
}

/// A URL a node can dial: a node URL, `https://host[:port]`.
///
/// # Errors
///
/// What is wrong with it.
pub fn parse_relay_url(text: &str) -> Result<String, String> {
    if dassie_peer::wire::is_valid_url(text) {
        Ok(text.to_string())
    } else {
        Err(format!("{text:?} is not of the form https://host[:port]"))
    }
}

fn listen(env: &Env, variable: &'static str) -> Result<SocketAddr, EnvError> {
    let text = env.get(variable)?.unwrap_or(DEFAULT_LISTEN);
    text.parse().map_err(|_| EnvError {
        variable,
        problem: format!("{text:?} is not an address and port"),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> Result<Settings, EnvError> {
        Settings::from_env(&Env::from_pairs(pairs.iter().copied()))
    }

    #[test]
    fn defaults_serve_the_default_domain_on_443() {
        let settings = settings(&[]).unwrap();
        assert_eq!(settings.domain.as_str(), DEFAULT_FRONT_DOOR_DOMAIN);
        assert_eq!(settings.https_listen.port(), 443);
        assert_eq!(settings.quic_listen.port(), 443);
        assert_eq!(settings.key_file, None);
        assert_eq!(settings.relay_url(), "https://relay.dassiepay.net");
    }

    #[test]
    fn the_key_file_defaults_to_the_state_directory() {
        let settings = settings(&[("STATE_DIRECTORY", "/var/lib/fd:/other")]).unwrap();
        assert_eq!(
            settings.key_file,
            Some(PathBuf::from("/var/lib/fd/front-door.key"))
        );
    }

    #[test]
    fn bad_values_name_their_variable() {
        for (variable, value) in [
            ("DASSIE_FRONT_DOOR_DOMAIN", "net"),
            ("DASSIE_FRONT_DOOR_DOMAIN", "not a name"),
            ("DASSIE_FRONT_DOOR_HTTPS_LISTEN", "443"),
            ("DASSIE_FRONT_DOOR_QUIC_LISTEN", "localhost:443"),
            ("DASSIE_FRONT_DOOR_RELAY_URL", "http://relay.example"),
        ] {
            let error = settings(&[(variable, value)]).unwrap_err();
            assert_eq!(error.variable, variable, "{value}");
        }
    }
}
