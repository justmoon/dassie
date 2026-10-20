//! `dassie status` and `dassie setup-url`: how the node is doing, for its
//! owner, over the IPC socket.
//!
//! `status` answers in a few lines what the owner would otherwise piece
//! together from `systemctl`, the journal and the wallet: is it running,
//! which version, its address and certificate, setup, the network, the
//! ledgers and the last update. It names no secret, so its output can be
//! pasted where the owner asks for help; that is why the setup URL has a
//! command of its own.

use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};

use crate::client::{CallError, Client};
use crate::update;

/// Days before a certificate's end at which `status` warns.
const EXPIRY_WARNING_DAYS: i64 = 14;
const DAY_MS: i64 = 86_400_000;

/// Runs `dassie status`. Fails only if the daemon cannot be reached.
pub async fn status(env: &Env) -> Result<(), String> {
    let mut client = match Client::connect(env).await {
        Ok(client) => client,
        Err(error) => {
            println!("Dassie is not running.");
            line("Updates", &updates());
            return Err(error.message);
        }
    };
    let now_ms = now_ms();
    let node = match client
        .call(Method::GetStatus(proto::GetStatusRequest {}))
        .await
    {
        Ok(response::Method::GetStatus(node)) => node,
        other => return Err(unexpected(other)),
    };
    let basic = match client
        .call(Method::GetBasicState(proto::GetBasicStateRequest {}))
        .await
    {
        Ok(response::Method::GetBasicState(basic)) => basic,
        other => return Err(unexpected(other)),
    };
    println!(
        "Dassie {} is running, for {} (since {}).",
        node.version,
        duration(now_ms.saturating_sub(node.started_at_unix_ms)),
        date(node.started_at_unix_ms)
    );
    line("Address", &address(&basic, node.https_port));
    line("Certificate", &certificate(&node, now_ms));
    let set_up = basic.state != i32::from(proto::NodeState::Uninitialized);
    if set_up {
        line("Setup", &format!("done; the node ID is {}", basic.node_id));
        line("Network", &network(&mut client).await);
        line("Ledgers", &ledgers(&mut client).await);
    } else {
        line(
            "Setup",
            "not done yet: `sudo dassie setup-url` prints the link to open in a browser",
        );
    }
    line("Updates", &updates());
    Ok(())
}

/// Runs `dassie setup-url`: prints the link that completes setup.
pub async fn setup_url(env: &Env) -> Result<(), String> {
    let mut client = Client::connect(env).await.map_err(|error| error.message)?;
    match client
        .call(Method::GetSetupUrl(proto::GetSetupUrlRequest {}))
        .await
    {
        Ok(response::Method::GetSetupUrl(setup)) => {
            println!("{}", setup.url);
            Ok(())
        }
        Err(CallError {
            code: Some(proto::ErrorCode::FailedPrecondition),
            ..
        }) => Err("The node is set up already: log in to the wallet with your passphrase.".into()),
        other => Err(unexpected(other)),
    }
}

fn unexpected(result: Result<response::Method, CallError>) -> String {
    match result {
        Err(error) => error.message,
        Ok(_) => "The daemon answered something this CLI does not understand".into(),
    }
}

/// Prints `label` and `text`, aligned with the other lines.
fn line(label: &str, text: &str) {
    println!("  {label:<12} {text}");
}

fn address(basic: &proto::GetBasicStateResponse, https_port: u32) -> String {
    if basic.hostname.is_empty() {
        return "none yet: `sudo dassie init` sets the node's domain name".into();
    }
    if https_port == 443 {
        format!("https://{}/", basic.hostname)
    } else {
        format!("https://{}:{https_port}/", basic.hostname)
    }
}

fn certificate(node: &proto::GetStatusResponse, now_ms: i64) -> String {
    let Some(certificate) = &node.certificate else {
        return "unknown".into();
    };
    let until = certificate.not_after_unix_ms;
    let mut text = match proto::CertificateSource::try_from(certificate.source) {
        Ok(proto::CertificateSource::SelfSigned) => {
            return "a temporary one the node made, which browsers warn about: \
                    `sudo dassie init` gets a real one"
                .into();
        }
        Ok(proto::CertificateSource::Acme) => {
            format!("valid until {}; the node renews it by itself", date(until))
        }
        _ => format!(
            "installed by hand, valid until {}; the node does not renew it",
            date(until)
        ),
    };
    if until <= now_ms {
        text.push_str(". IT HAS EXPIRED: browsers and peers refuse the node");
    } else if until.saturating_sub(now_ms) < EXPIRY_WARNING_DAYS * DAY_MS {
        text.push_str(&format!(". It ends in {}", duration(until - now_ms)));
    }
    if let Some(acme) = &node.acme
        && !acme.last_error.is_empty()
    {
        text.push_str(&format!(". The last renewal failed: {}", acme.last_error));
    }
    text
}

async fn network(client: &mut Client) -> String {
    let registration = match client
        .call(Method::GetRegistrationStatus(
            proto::GetRegistrationStatusRequest {},
        ))
        .await
    {
        Ok(response::Method::GetRegistrationStatus(registration)) => registration,
        other => return format!("unknown ({})", unexpected(other)),
    };
    if registration.bootstrap.is_empty() {
        return "no bootstrap nodes are set (DASSIE_BOOTSTRAP_NODES), so the node runs alone"
            .into();
    }
    let listed = registration
        .bootstrap
        .iter()
        .filter(|bootstrap| bootstrap.listed)
        .count();
    let total = registration.bootstrap.len();
    let membership = if registration.member {
        format!("a member of the network, listed by {listed} of {total} bootstrap nodes")
    } else {
        format!("not a member yet, listed by {listed} of {total} bootstrap nodes")
    };
    let peers = match client
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await
    {
        Ok(response::Method::ListPeers(list)) => list
            .peers
            .iter()
            .filter(|peer| peer.state == "peered")
            .count()
            .to_string(),
        _ => "unknown".into(),
    };
    format!("{membership}; peers: {peers}")
}

async fn ledgers(client: &mut Client) -> String {
    let list = match client
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    {
        Ok(response::Method::ListLedgers(list)) => list,
        other => return format!("unknown ({})", unexpected(other)),
    };
    if list.ledgers.is_empty() {
        return "none yet: add one in the wallet".into();
    }
    list.ledgers
        .iter()
        .map(|ledger| {
            format!(
                "{}: {} {}, {}",
                ledger.id,
                amount(ledger.owner_balance, ledger.asset_scale),
                ledger.asset_code,
                connection(ledger)
            )
        })
        .collect::<Vec<_>>()
        .join(&format!("\n  {:<12} ", ""))
}

fn connection(ledger: &proto::Ledger) -> String {
    match proto::LedgerConnection::try_from(ledger.connection) {
        Ok(proto::LedgerConnection::Synced) => "in sync".into(),
        Ok(proto::LedgerConnection::Connecting) => "connecting".into(),
        Ok(proto::LedgerConnection::Syncing) => "catching up".into(),
        Ok(proto::LedgerConnection::Unfunded) => {
            format!("waiting for funds at {}", ledger.deposit_address)
        }
        Ok(proto::LedgerConnection::Failed) => format!("failed: {}", ledger.connection_problem),
        _ => "unknown".into(),
    }
}

fn updates() -> String {
    let Ok(install) = update::locate() else {
        return "this binary was not installed by the installer, so it is not updated".into();
    };
    let Some(last) = update::last_update(&install) else {
        return "none yet; the node checks every night".into();
    };
    let at_ms = i64::try_from(last.at_unix_ms).unwrap_or(i64::MAX);
    let when = date(at_ms);
    let target = last.target.as_deref().unwrap_or("?");
    let problem = last.problem.as_deref().unwrap_or("no reason given");
    match last.outcome.as_str() {
        "up-to-date" => format!("checked {when}: up to date"),
        "updated" => format!("{when}: updated to {target}"),
        "rolled-back" => {
            format!("{when}: {target} did not start ({problem}), so the version before runs again")
        }
        "skipped" => format!(
            "checked {when}: {target} did not start here before, so the node waits for a newer \
             version (`dassie update --force` tries it again)"
        ),
        _ => format!("{when}: the update failed: {problem}"),
    }
}

/// `value` units at `scale`, as a decimal without trailing zeros.
fn amount(value: i64, scale: u32) -> String {
    let digits = value.unsigned_abs().to_string();
    let scale = usize::try_from(scale).unwrap_or(usize::MAX);
    let sign = if value < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale.saturating_add(1));
    let (whole, fraction) = padded.split_at(padded.len() - scale);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

/// A length of time, roughly, in the largest unit that fits.
fn duration(ms: i64) -> String {
    let minutes = ms / 60_000;
    let plural = |n: i64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    match minutes {
        ..1 => "less than a minute".into(),
        1..120 => plural(minutes, "minute"),
        120..2880 => plural(minutes / 60, "hour"),
        _ => plural(minutes / 1440, "day"),
    }
}

/// A moment, in UTC, to the minute.
fn date(unix_ms: i64) -> String {
    let nanos = i128::from(unix_ms) * 1_000_000;
    match time::OffsetDateTime::from_unix_timestamp_nanos(nanos) {
        Ok(at) => format!(
            "{}-{:02}-{:02} {:02}:{:02} UTC",
            at.year(),
            u8::from(at.month()),
            at.day(),
            at.hour(),
            at.minute()
        ),
        Err(_) => "an unknown time".into(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(amount(0, 6), "0");
        assert_eq!(amount(99_500_000, 6), "99.5");
        assert_eq!(amount(5, 2), "0.05");
        assert_eq!(amount(-1_230, 2), "-12.3");
        assert_eq!(amount(42, 0), "42");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(30_000), "less than a minute");
        assert_eq!(duration(60_000), "1 minute");
        assert_eq!(duration(3 * 3_600_000), "3 hours");
        assert_eq!(duration(3 * DAY_MS), "3 days");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01 00:00 UTC");
        assert_eq!(
            date(1_792_022_400_000 + 754 * 60_000),
            "2026-10-15 12:34 UTC"
        );
    }
}
