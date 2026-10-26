//! `dassie hub found`, `foundings`, `drop` and `payout`: founding a
//! settlement hub as its sponsor (spec/areas/hubs.md#founding-a-hub), and
//! the payout bundles the node keeps of its hub
//! (spec/areas/hubs.md#payout-bundles), over the IPC socket.
//!
//! - `dassie hub found <node ID>@<host:port>... [--terms FILE]
//!   [--escrow-funding XRP] [--xrpl URL]` founds a hub with these
//!   validators, every one of which must accept. Without `--terms` the
//!   terms are the starting ones; a file of JSON sets what differs.
//! - `dassie hub foundings` shows each founding: which validators
//!   accepted, signed and run the hub, and whether its escrow is set up
//!   and checked; once fixed, the rules a member's scheme takes.
//! - `dassie hub drop <nonce>` drops a founding.
//! - `dassie hub payout` shows the payout bundles kept of the node's hub,
//!   and, once a quorum's sign one set, its transactions to submit.

use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};
use lexopt::prelude::*;

use crate::client::Client;
use crate::status::{date, entries, line, unexpected};

/// The largest file of terms read.
const MAX_TERMS_FILE_BYTES: u64 = 8 * 1024;

/// What `dassie hub found` and its kin are asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Found a hub.
    Found {
        /// The validators, each `<node ID>@<host:port>`.
        validators: Vec<(String, String)>,
        /// The file of terms that differ from the starting ones.
        terms: Option<String>,
        /// What funds the escrow, in drops.
        funding_drops: Option<u64>,
        /// The XRPL server.
        xrpl: Option<String>,
    },
    /// Show the foundings.
    List,
    /// Drop the founding of this nonce.
    Drop(String),
    /// Show the payout bundles.
    Payout,
}

/// Reads the arguments of `word`, one of `found`, `foundings`, `drop` and
/// `payout`.
///
/// # Errors
///
/// A command line that is not one of these.
pub fn args(word: &str, parser: &mut lexopt::Parser) -> Result<Command, lexopt::Error> {
    let command = match word {
        "foundings" => Command::List,
        "payout" => Command::Payout,
        "drop" => Command::Drop(parser.value()?.string()?),
        "found" => return found_args(parser),
        other => return Err(format!("unknown hub command {other:?}").into()),
    };
    if let Some(extra) = parser.next()? {
        return Err(extra.unexpected());
    }
    Ok(command)
}

fn found_args(parser: &mut lexopt::Parser) -> Result<Command, lexopt::Error> {
    let mut validators = Vec::new();
    let (mut terms, mut funding_drops, mut xrpl) = (None, None, None);
    while let Some(arg) = parser.next()? {
        match arg {
            Long("terms") => terms = Some(parser.value()?.string()?),
            Long("escrow-funding") => {
                let units = crate::hub::xrp(&parser.value()?.string()?)?;
                funding_drops = Some(units / 1_000);
            }
            Long("xrpl") => xrpl = Some(parser.value()?.string()?),
            Value(validator) => {
                let validator = validator.string()?;
                let (node, endpoint) = validator.split_once('@').ok_or_else(|| {
                    format!("{validator:?} is not a validator: <node ID>@<host:port>")
                })?;
                validators.push((node.to_string(), endpoint.to_string()));
            }
            other => return Err(other.unexpected()),
        }
    }
    if validators.is_empty() {
        return Err("name the validators: <node ID>@<host:port> ...".into());
    }
    Ok(Command::Found {
        validators,
        terms,
        funding_drops,
        xrpl,
    })
}

/// Runs the command.
pub async fn run(env: &Env, command: Command) -> Result<(), String> {
    let method = match command {
        Command::Found {
            validators,
            terms,
            funding_drops,
            xrpl,
        } => Method::FoundHub(proto::FoundHubRequest {
            validators: validators
                .into_iter()
                .map(|(node, endpoint)| proto::HubFoundingInvite { node, endpoint })
                .collect(),
            terms: terms
                .as_deref()
                .map(read_terms)
                .transpose()?
                .unwrap_or_default(),
            escrow_funding_drops: funding_drops,
            xrpl_server: xrpl.unwrap_or_default(),
        }),
        Command::List => Method::ListHubFoundings(proto::ListHubFoundingsRequest {}),
        Command::Drop(nonce) => Method::DropHubFounding(proto::DropHubFoundingRequest { nonce }),
        Command::Payout => Method::GetHubPayout(proto::GetHubPayoutRequest {}),
    };
    let mut client = Client::connect(env).await.map_err(|error| error.message)?;
    match client.call(method).await {
        Ok(response::Method::FoundHub(proto::FoundHubResponse {
            founding: Some(founding),
        })) => {
            println!(
                "Founding the hub {}: `dassie hub foundings` shows how far it is.",
                founding.nonce
            );
            show(&founding);
        }
        Ok(response::Method::ListHubFoundings(list)) if list.foundings.is_empty() => {
            println!("The node founds no hub.");
        }
        Ok(response::Method::ListHubFoundings(list)) => list.foundings.iter().for_each(show),
        Ok(response::Method::DropHubFounding(_)) => println!("Dropped."),
        Ok(response::Method::GetHubPayout(payout)) => show_payout(&payout),
        other => return Err(unexpected(other)),
    }
    Ok(())
}

/// The terms in the file at `path`.
fn read_terms(path: &str) -> Result<String, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let mut text = String::new();
    file.take(MAX_TERMS_FILE_BYTES.saturating_add(1))
        .read_to_string(&mut text)
        .map_err(|error| format!("{path}: {error}"))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_TERMS_FILE_BYTES {
        return Err(format!(
            "{path} is longer than {MAX_TERMS_FILE_BYTES} bytes"
        ));
    }
    Ok(text)
}

/// A founding, as `dassie hub foundings` shows it.
fn show(founding: &proto::HubFounding) {
    println!();
    line("Founding", &founding.nonce);
    line("Stage", stage(founding));
    line("Started", &date(founding.started_unix_ms));
    if !founding.hub.is_empty() {
        line("Hub", &founding.hub);
    }
    line("Escrow", &escrow(founding));
    for validator in &founding.validators {
        line("Validator", &invited(validator));
    }
    if !founding.problem.is_empty() {
        line("Problem", &founding.problem);
    }
    if !founding.rules.is_empty() {
        line("Rules", &founding.rules);
    }
}

fn stage(founding: &proto::HubFounding) -> &'static str {
    match proto::HubFoundingStage::try_from(founding.stage) {
        Ok(proto::HubFoundingStage::Accepting) => "asking the validators to accept the terms",
        Ok(proto::HubFoundingStage::Signing) => "the rules are fixed; asking for the genesis",
        Ok(proto::HubFoundingStage::Certifying) => "the genesis is certified; starting the hub",
        Ok(proto::HubFoundingStage::Escrow) => "the hub runs; setting up its escrow",
        Ok(proto::HubFoundingStage::Founded) => "founded: the escrow is checked",
        _ => "unknown",
    }
}

fn escrow(founding: &proto::HubFounding) -> String {
    let state = if founding.escrow_set_up {
        "set up"
    } else if founding.escrow_funded {
        "funded, being set up"
    } else {
        "not funded yet"
    };
    let checked = founding
        .validators
        .iter()
        .filter(|validator| validator.escrow_checked)
        .count();
    format!(
        "{}, {state}; {checked} of {} validators say it is checked",
        founding.escrow,
        founding.validators.len()
    )
}

fn invited(validator: &proto::HubFoundingValidator) -> String {
    let steps = [
        (validator.accepted, "accepted"),
        (validator.signed, "signed the genesis"),
        (validator.running, "runs the hub"),
    ];
    let done: Vec<&str> = steps
        .iter()
        .filter(|(done, _)| *done)
        .map(|(_, what)| *what)
        .collect();
    let mut text = format!("{} at {}", validator.node, validator.endpoint);
    text.push_str(&format!(
        ": {}",
        if done.is_empty() {
            "no answer yet".to_string()
        } else {
            entries(done.iter().map(ToString::to_string))
        }
    ));
    if !validator.refusal.is_empty() {
        text.push_str(&format!("; refused: {}", validator.refusal));
    }
    text
}

/// The payout bundles, as `dassie hub payout` shows them.
fn show_payout(payout: &proto::GetHubPayoutResponse) {
    line("Hub", &payout.hub);
    if payout.bundles.is_empty() {
        line("Payout", "no validator signed a payout set: the hub runs");
    }
    for bundle in &payout.bundles {
        line(
            "Signed",
            &format!(
                "validator {} signed the payout of block {}, {} transactions",
                u64::from(bundle.validator).saturating_add(1),
                bundle.height,
                bundle.transactions
            ),
        );
    }
    match payout.ready_height {
        Some(height) => {
            line(
                "Ready",
                &format!(
                    "a quorum signed the payout of block {height}: submit these to the XRP Ledger in order"
                ),
            );
            for transaction in &payout.transactions {
                println!("{transaction}");
            }
        }
        None if !payout.bundles.is_empty() => line(
            "Ready",
            &format!("not yet: {} validators must sign one set", payout.quorum),
        ),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<Command, lexopt::Error> {
        let mut parser = lexopt::Parser::from_args(words.iter().skip(1).copied());
        args(words[0], &mut parser)
    }

    #[test]
    fn the_command_line_names_validators_and_terms() {
        let found = parse(&["found", "dA@v1.example:443", "dB@v2.example:443"]);
        assert_eq!(
            found.unwrap(),
            Command::Found {
                validators: vec![
                    ("dA".into(), "v1.example:443".into()),
                    ("dB".into(), "v2.example:443".into())
                ],
                terms: None,
                funding_drops: None,
                xrpl: None,
            }
        );
        let Ok(Command::Found {
            terms,
            funding_drops,
            ..
        }) = parse(&[
            "found",
            "dA@v:1",
            "--terms",
            "t.json",
            "--escrow-funding",
            "2.5",
        ])
        else {
            panic!("found");
        };
        assert_eq!(terms.as_deref(), Some("t.json"));
        assert_eq!(funding_drops, Some(2_500_000));
        assert!(parse(&["found"]).is_err());
        assert!(parse(&["found", "nohost"]).is_err());
        assert!(parse(&["found", "dA@v:1", "--testnet-defaults"]).is_err());
        assert_eq!(parse(&["foundings"]).unwrap(), Command::List);
        assert_eq!(parse(&["drop", "ab"]).unwrap(), Command::Drop("ab".into()));
        assert_eq!(parse(&["payout"]).unwrap(), Command::Payout);
        assert!(parse(&["payout", "x"]).is_err());
    }
}
