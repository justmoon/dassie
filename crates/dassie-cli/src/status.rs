//! `dassie status` and `dassie setup-url`: how the node is doing, for its
//! owner, over the IPC socket.
//!
//! `status` answers in a few lines what the owner would otherwise piece
//! together from `systemctl`, the journal and the wallet: is it running,
//! which version, its address and certificate, setup, the network, the
//! ledgers, the rate feed and the last update. A periphery node is described as one: it
//! registers nowhere and needs no domain name, so instead of its membership
//! it names its uplinks and its relay, and its addresses are its names for
//! Open Payments. It names no secret, so its output can be pasted where the
//! owner asks for help; that is why the setup URL has a command of its own.

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
    line("Address", &address(&basic, &node));
    line("Certificate", &certificate(&node, now_ms));
    let set_up = basic.state != i32::from(proto::NodeState::Uninitialized);
    if set_up {
        line("Setup", &format!("done; the node ID is {}", basic.node_id));
        let peers = peers(&mut client).await;
        let network = network(&mut client, peers.as_deref(), node.periphery).await;
        line("Network", &network);
        if node.periphery {
            let nodes = nodes(&mut client).await;
            line("Uplinks", &uplinks(peers.as_deref(), &nodes, now_ms));
        }
        if let Some(relay) = relay(node.relay.as_ref(), node.periphery) {
            line("Relay", &relay);
        }
        line("Ledgers", &ledgers(&mut client).await);
        if let Some(hub) = crate::hub::summary(&mut client).await {
            line("Hub", &hub);
        }
        if let Some(feed) = feed(&node) {
            line("Rate feed", &feed);
        }
        for hub in &node.validating {
            line("Validating", &validating(hub, now_ms));
        }
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

/// Prints a pairing code (spec/areas/rpc.md#paired-devices).
pub async fn pair(env: &Env) -> Result<(), String> {
    let mut client = Client::connect(env).await.map_err(|error| error.message)?;
    match client
        .call(Method::StartPairing(proto::StartPairingRequest {}))
        .await
    {
        Ok(response::Method::StartPairing(started)) => {
            println!("{}", started.code);
            eprintln!(
                "Enter this code in the app within five minutes. It works once, and \
                 `dassie pair` again replaces it. The wallet on localhost shows it as a QR code."
            );
            Ok(())
        }
        Err(CallError {
            code: Some(proto::ErrorCode::FailedPrecondition),
            message,
        }) => Err(message),
        other => Err(unexpected(other)),
    }
}

pub(crate) fn unexpected(result: Result<response::Method, CallError>) -> String {
    match result {
        Err(error) => error.message,
        Ok(_) => "The daemon answered something this CLI does not understand".into(),
    }
}

/// Prints `label` and `text`, aligned with the other lines.
pub(crate) fn line(label: &str, text: &str) {
    println!("  {label:<12} {text}");
}

/// Joins the entries of one line, each on a line of its own, aligned under
/// the first.
pub(crate) fn entries(texts: impl IntoIterator<Item = String>) -> String {
    texts
        .into_iter()
        .collect::<Vec<_>>()
        .join(&format!("\n  {:<12} ", ""))
}

/// Where the node is reached: at its hostname, or, on a periphery node,
/// which needs no domain name, at its names for Open Payments, which serve
/// nothing else.
fn address(basic: &proto::GetBasicStateResponse, node: &proto::GetStatusResponse) -> String {
    if node.periphery {
        if node.open_payments_origins.is_empty() {
            return "none yet: a periphery node is named once it is set up".into();
        }
        let origins = node.open_payments_origins.iter();
        return entries(origins.map(|origin| format!("{origin}/ (Open Payments only)")));
    }
    if basic.hostname.is_empty() {
        return "none yet: `sudo dassie init` sets the node's domain name".into();
    }
    if node.https_port == 443 {
        format!("https://{}/", basic.hostname)
    } else {
        format!("https://{}:{}/", basic.hostname, node.https_port)
    }
}

fn certificate(node: &proto::GetStatusResponse, now_ms: i64) -> String {
    let Some(certificate) = &node.certificate else {
        return "unknown".into();
    };
    let until = certificate.not_after_unix_ms;
    let mut text = match proto::CertificateSource::try_from(certificate.source) {
        Ok(proto::CertificateSource::SelfSigned) if node.periphery => {
            return "a temporary one the node made, which browsers warn about; \
                    a periphery node needs no other"
                .into();
        }
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

/// The node's peerings, or None if the daemon would not say.
async fn peers(client: &mut Client) -> Option<Vec<proto::NetworkPeer>> {
    match client
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await
    {
        Ok(response::Method::ListPeers(list)) => Some(list.peers),
        _ => None,
    }
}

/// The node table, for its peers' URLs; empty if the daemon would not say.
async fn nodes(client: &mut Client) -> Vec<proto::NetworkNode> {
    match client
        .call(Method::ListNodes(proto::ListNodesRequest {}))
        .await
    {
        Ok(response::Method::ListNodes(list)) => list.nodes,
        _ => Vec::new(),
    }
}

async fn network(
    client: &mut Client,
    peers: Option<&[proto::NetworkPeer]>,
    periphery: bool,
) -> String {
    match client
        .call(Method::GetRegistrationStatus(
            proto::GetRegistrationStatusRequest {},
        ))
        .await
    {
        Ok(response::Method::GetRegistrationStatus(registration)) => {
            membership(&registration, peers, periphery)
        }
        other => format!("unknown ({})", unexpected(other)),
    }
}

/// The node's place in the network. A periphery node registers nowhere, as
/// it should, so its registrations say nothing about it
/// (spec/areas/network.md#periphery-nodes).
fn membership(
    registration: &proto::GetRegistrationStatusResponse,
    peers: Option<&[proto::NetworkPeer]>,
    periphery: bool,
) -> String {
    if registration.bootstrap.is_empty() {
        return "no bootstrap nodes are set (DASSIE_BOOTSTRAP_NODES), so the node runs alone"
            .into();
    }
    if periphery {
        return "a periphery node: it registers with no bootstrap node, and reaches the \
                network through its uplinks"
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
    let peers = peers.map_or_else(
        || "unknown".into(),
        |peers| peered(peers).count().to_string(),
    );
    format!("{membership}; peers: {peers}")
}

fn peered(peers: &[proto::NetworkPeer]) -> impl Iterator<Item = &proto::NetworkPeer> {
    peers.iter().filter(|peer| peer.state == "peered")
}

/// A periphery node's uplinks, which are its peers, and whether each is
/// connected. Without one, nothing reaches the node.
fn uplinks(
    peers: Option<&[proto::NetworkPeer]>,
    nodes: &[proto::NetworkNode],
    now_ms: i64,
) -> String {
    let Some(peers) = peers else {
        return "unknown".into();
    };
    let uplinks: Vec<String> = peered(peers)
        .map(|peer| {
            let url = nodes
                .iter()
                .find(|node| node.id == peer.node)
                .map_or("", |node| node.url.as_str());
            let state = if peer.online {
                "connected".into()
            } else if peer.last_seen_unix_ms > 0 {
                let silent_ms = now_ms.saturating_sub(peer.last_seen_unix_ms);
                format!("not connected, last heard from {} ago", duration(silent_ms))
            } else {
                "not connected yet".into()
            };
            format!("{}, {state}", who(&peer.node, url))
        })
        .collect();
    if uplinks.is_empty() {
        return "none yet, so no payment reaches the node: it peers with core nodes that \
                share one of its ledgers"
            .into();
    }
    entries(uplinks)
}

/// The relay the node chose, if any. A periphery node without one says so:
/// Open Payments clients reach it only through a relay
/// (spec/areas/node.md#a-chosen-relay). A node that is still connecting to
/// its relay, right after it starts or after a brief drop, gets no hint:
/// only one that stays unreachable does.
fn relay(relay: Option<&proto::ChosenRelay>, periphery: bool) -> Option<String> {
    let Some(relay) = relay else {
        return periphery.then(|| {
            "none (DASSIE_RELAY is empty), so Open Payments clients cannot reach the node".into()
        });
    };
    let who = who(&relay.node, &relay.url);
    let connection = relay_connection(relay);
    Some(if connection == proto::RelayConnection::Connected {
        format!("{who}, connected")
    } else if connection == proto::RelayConnection::Connecting {
        format!("{who}, connecting")
    } else {
        format!(
            "{who}, not connected, so Open Payments clients cannot reach the node through it: \
             check that DASSIE_RELAY is the entry the front door logs, or the relay's ID, URL \
             and key, and that the node can reach it over UDP"
        )
    })
}

/// How the node stands with its relay. A daemon that predates
/// `connection`, or knows a state this CLI does not, still says whether it
/// is connected; not connected then reads as unreachable, as it did before.
fn relay_connection(relay: &proto::ChosenRelay) -> proto::RelayConnection {
    match proto::RelayConnection::try_from(relay.connection) {
        Ok(proto::RelayConnection::Unspecified) | Err(_) if relay.connected => {
            proto::RelayConnection::Connected
        }
        Ok(proto::RelayConnection::Unspecified) | Err(_) => proto::RelayConnection::Unreachable,
        Ok(connection) => connection,
    }
}

/// The rate feed: whether the node serves it and where, how many it goes
/// to, and its counters since the node started
/// (spec/areas/rate-feed.md#what-the-owner-sees). A periphery node
/// publishes none, by design, so it gets no line.
fn feed(node: &proto::GetStatusResponse) -> Option<String> {
    let Some(feed) = &node.feed else {
        return (!node.periphery).then(|| {
            "none: the node has no UDP socket for it (under systemd, `dassie-feed.socket`)".into()
        });
    };
    if !feed.serving {
        return (!node.periphery).then(|| {
            format!(
                "not served yet: the node publishes it at {} once it has a domain name",
                feed.address
            )
        });
    }
    let subscribers = plural(u64::from(feed.subscribers), "subscriber");
    Some(entries([
        format!("served at {}, to {subscribers}", feed.address),
        format!(
            "published {}; sent {}, {} failed, {} out of time",
            plural(feed.published, "feed"),
            plural(feed.sent, "datagram"),
            feed.send_failed,
            feed.unsent
        ),
        format!(
            "took {}: {} challenged, {} subscribed, {} renewed, {} ignored; {} lapsed",
            plural(feed.received, "datagram"),
            feed.challenged,
            feed.subscribed,
            feed.renewed,
            feed.ignored,
            feed.expired
        ),
        format!(
            "refused {} for a full table, {} for a client's share",
            feed.refused_full, feed.refused_client
        ),
    ]))
}

/// How long a hub may go without a final block before status says so: a
/// block is final every fraction of a second while the hub runs.
const HUB_QUIET_MS: i64 = 60_000;

/// A hub the node validates (spec/areas/hubs.md#the-validator-in-the-node):
/// which validator it is, the last final block, and a hint only where
/// something is wrong.
fn validating(hub: &proto::HubValidator, now_ms: i64) -> String {
    let short = hub.hub.get(..16).unwrap_or(&hub.hub);
    // The daemon names the set's size as the index of a node no longer in
    // it.
    let which = if hub.index < hub.validators {
        format!(
            "validator {} of {}",
            u64::from(hub.index).saturating_add(1),
            hub.validators
        )
    } else {
        format!("no longer one of its {} validators", hub.validators)
    };
    let mut text = format!(
        "hub {short}…, {which}, block {} in view {}",
        hub.height, hub.view
    );
    let quiet_ms = now_ms.saturating_sub(hub.final_unix_ms);
    let phase = proto::HubPhase::try_from(hub.phase).unwrap_or(proto::HubPhase::Unspecified);
    match phase {
        proto::HubPhase::ShuttingDown => text.push_str("; shutting down"),
        proto::HubPhase::Ended => text.push_str("; ended: its payout set is signed"),
        proto::HubPhase::Running | proto::HubPhase::Unspecified => {}
    }
    if hub.stalled {
        text.push_str(
            "; stalled: it never votes in the hub again, and signed the payout set of its last block",
        );
    } else if phase != proto::HubPhase::Ended && quiet_ms > HUB_QUIET_MS {
        text.push_str(&format!(
            "; no block final for {}: check that its co-validators reach it over QUIC",
            duration(quiet_ms)
        ));
    }
    if !hub.escrow_checked {
        text.push_str("; its escrow is not checked yet, so no deposit is credited");
    }
    if !hub.xrpl_connected {
        text.push_str("; not following the escrow: its XRPL server cannot be reached");
    }
    text
}

/// `n` of `unit`, in the plural unless one.
fn plural(n: u64, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

/// A node by its ID, and its URL if it has one.
fn who(id: &str, url: &str) -> String {
    if url.is_empty() {
        id.into()
    } else {
        format!("{id} at {url}")
    }
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
    entries(list.ledgers.iter().map(|ledger| {
        format!(
            "{}: {} {}, {}",
            ledger.id,
            amount(ledger.owner_balance, ledger.asset_scale),
            ledger.asset_code,
            connection(ledger)
        )
    }))
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
        "skipped" => {
            let why = last
                .rolled_back
                .as_ref()
                .and_then(|rolled_back| rolled_back.problem.as_deref())
                .unwrap_or("no reason given");
            format!(
                "checked {when}: {target} did not start here before ({why}), so the node waits \
                 for a newer version (`dassie update --force` tries it again)"
            )
        }
        _ => format!("{when}: the update failed: {problem}"),
    }
}

/// `value` units at `scale`, as a decimal without trailing zeros.
pub(crate) fn amount(value: i64, scale: u32) -> String {
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
pub(crate) fn date(unix_ms: i64) -> String {
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

    const UPLINK: &str = "dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk";
    const NAME: &str = "https://gstpaogawdidju5a6m4lolevaes3cm4dj7omffq.dassiepay.net";

    fn periphery() -> proto::GetStatusResponse {
        proto::GetStatusResponse {
            https_port: 443,
            periphery: true,
            ..Default::default()
        }
    }

    fn registration(listed: usize, total: usize) -> proto::GetRegistrationStatusResponse {
        proto::GetRegistrationStatusResponse {
            bootstrap: (0..total)
                .map(|index| proto::BootstrapStatus {
                    listed: index < listed,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn peer(node: &str, online: bool, last_seen_unix_ms: i64) -> proto::NetworkPeer {
        proto::NetworkPeer {
            node: node.into(),
            state: "peered".into(),
            online,
            last_seen_unix_ms,
            ..Default::default()
        }
    }

    #[test]
    fn a_core_node_is_at_its_hostname() {
        let basic = proto::GetBasicStateResponse {
            hostname: "node.example".into(),
            ..Default::default()
        };
        let node = proto::GetStatusResponse {
            https_port: 443,
            ..Default::default()
        };
        assert_eq!(address(&basic, &node), "https://node.example/");
        let node = proto::GetStatusResponse {
            https_port: 8443,
            ..node
        };
        assert_eq!(address(&basic, &node), "https://node.example:8443/");
        let none = proto::GetBasicStateResponse::default();
        assert_eq!(
            address(&none, &node),
            "none yet: `sudo dassie init` sets the node's domain name"
        );
    }

    // A periphery node needs no domain name: it is not sent to `dassie init`.
    #[test]
    fn a_periphery_node_is_at_its_names() {
        let basic = proto::GetBasicStateResponse::default();
        assert_eq!(
            address(&basic, &periphery()),
            "none yet: a periphery node is named once it is set up"
        );
        let node = proto::GetStatusResponse {
            open_payments_origins: vec![NAME.into(), "https://pay.example".into()],
            ..periphery()
        };
        assert_eq!(
            address(&basic, &node),
            format!(
                "{NAME}/ (Open Payments only)\n               \
                 https://pay.example/ (Open Payments only)"
            )
        );
        let certificate = proto::Certificate {
            source: proto::CertificateSource::SelfSigned.into(),
            ..Default::default()
        };
        let node = proto::GetStatusResponse {
            certificate: Some(certificate),
            ..node
        };
        let text = super::certificate(&node, 0);
        assert!(text.ends_with("a periphery node needs no other"), "{text}");
    }

    #[test]
    fn a_core_node_says_how_many_bootstrap_nodes_list_it() {
        let peers = [peer(UPLINK, true, 1), peer("dOther", true, 1)];
        assert_eq!(
            membership(&registration(0, 3), Some(&peers), false),
            "not a member yet, listed by 0 of 3 bootstrap nodes; peers: 2"
        );
        let member = proto::GetRegistrationStatusResponse {
            member: true,
            ..registration(2, 3)
        };
        assert_eq!(
            membership(&member, None, false),
            "a member of the network, listed by 2 of 3 bootstrap nodes; peers: unknown"
        );
    }

    // A periphery node registers nowhere, which is no fault: it does not read
    // as "not a member yet".
    #[test]
    fn a_periphery_node_is_not_described_by_its_registrations() {
        let text = membership(&registration(0, 3), Some(&[]), true);
        assert!(text.starts_with("a periphery node: "), "{text}");
        assert!(!text.contains("member"), "{text}");
        let alone = membership(&registration(0, 0), Some(&[]), true);
        assert!(alone.contains("runs alone"), "{alone}");
    }

    #[test]
    fn uplinks_and_whether_they_are_connected() {
        let nodes = [proto::NetworkNode {
            id: UPLINK.into(),
            url: "https://boot1.dassie.xyz".into(),
            ..Default::default()
        }];
        let now_ms = 10 * 60_000;
        let mut ending = peer("dEnding", true, 1);
        ending.state = "terminating".into();
        let peers = [
            peer(UPLINK, true, now_ms),
            peer("dSilent", false, now_ms - 5 * 60_000),
            peer("dNew", false, 0),
            ending,
        ];
        assert_eq!(
            uplinks(Some(&peers), &nodes, now_ms),
            format!(
                "{UPLINK} at https://boot1.dassie.xyz, connected\n               \
                 dSilent, not connected, last heard from 5 minutes ago\n               \
                 dNew, not connected yet"
            )
        );
        let none = uplinks(Some(&[]), &nodes, now_ms);
        assert!(
            none.starts_with("none yet, so no payment reaches the node"),
            "{none}"
        );
        assert_eq!(uplinks(None, &nodes, now_ms), "unknown");
    }

    #[test]
    fn the_chosen_relay() {
        let mut chosen = proto::ChosenRelay {
            node: "dRelay".into(),
            url: "https://relay.dassiepay.net".into(),
            connected: true,
            ..Default::default()
        };
        let connected = "dRelay at https://relay.dassiepay.net, connected";
        assert_eq!(relay(Some(&chosen), true).as_deref(), Some(connected));
        assert_eq!(relay(Some(&chosen), false).as_deref(), Some(connected));
        chosen.connected = false;
        let lost = relay(Some(&chosen), true).unwrap();
        assert!(
            lost.starts_with("dRelay at https://relay.dassiepay.net, not connected, so "),
            "{lost}"
        );
        // A front door logs the entry that names it; a plain node chosen as
        // a relay logs none, so the hint names what the entry holds too.
        assert!(
            lost.ends_with(
                ": check that DASSIE_RELAY is the entry the front door logs, or the relay's ID, \
                 URL and key, and that the node can reach it over UDP"
            ),
            "{lost}"
        );
        // A core node without one is reached at its hostname: no line.
        assert_eq!(relay(None, false), None);
        assert_eq!(
            relay(None, true).as_deref(),
            Some("none (DASSIE_RELAY is empty), so Open Payments clients cannot reach the node")
        );
    }

    // A daemon that says how it stands with its relay (above, one that says
    // only whether it is connected): connecting, right after a start or a
    // brief drop, is no fault and gets no hint; a relay that stays
    // unreachable does.
    #[test]
    fn the_chosen_relay_while_connecting() {
        use proto::RelayConnection::{Connected, Connecting, Unreachable};
        let chosen = |connection: proto::RelayConnection| proto::ChosenRelay {
            node: "dRelay".into(),
            url: "https://relay.dassiepay.net".into(),
            connected: connection == Connected,
            connection: connection.into(),
        };
        let text = |connection| relay(Some(&chosen(connection)), true).unwrap();
        let who = "dRelay at https://relay.dassiepay.net";
        assert_eq!(text(Connected), format!("{who}, connected"));
        assert_eq!(text(Connecting), format!("{who}, connecting"));
        let unreachable = text(Unreachable);
        assert!(
            unreachable.starts_with(&format!("{who}, not connected, so ")),
            "{unreachable}"
        );
        assert!(
            unreachable.contains("check that DASSIE_RELAY"),
            "{unreachable}"
        );
        // A state this CLI does not know, from a newer daemon, falls back to
        // whether it is connected.
        let mut newer = chosen(Connecting);
        newer.connection = 99;
        assert_eq!(relay(Some(&newer), true), Some(unreachable));
        newer.connected = true;
        assert_eq!(relay(Some(&newer), true), Some(format!("{who}, connected")));
    }

    // A core node's feed: where it is served, to how many, and its
    // counters; or why it is not.
    #[test]
    fn the_rate_feed() {
        let core = |feed: Option<proto::RateFeed>| proto::GetStatusResponse {
            feed,
            ..Default::default()
        };
        let served = proto::RateFeed {
            serving: true,
            address: "0.0.0.0:7768".into(),
            subscribers: 1,
            published: 12,
            sent: 30,
            send_failed: 2,
            unsent: 1,
            received: 40,
            challenged: 3,
            subscribed: 2,
            renewed: 33,
            refused_full: 1,
            refused_client: 0,
            ignored: 1,
            expired: 1,
        };
        assert_eq!(
            feed(&core(Some(served.clone()))).unwrap(),
            "served at 0.0.0.0:7768, to 1 subscriber\n               \
             published 12 feeds; sent 30 datagrams, 2 failed, 1 out of time\n               \
             took 40 datagrams: 3 challenged, 2 subscribed, 33 renewed, 1 ignored; 1 lapsed\n               \
             refused 1 for a full table, 0 for a client's share"
        );
        let waiting = proto::RateFeed {
            serving: false,
            subscribers: 0,
            ..served
        };
        assert_eq!(
            feed(&core(Some(waiting.clone()))).unwrap(),
            "not served yet: the node publishes it at 0.0.0.0:7768 once it has a domain name"
        );
        let none = feed(&core(None)).unwrap();
        assert!(
            none.starts_with("none: the node has no UDP socket"),
            "{none}"
        );
        // A periphery node publishes none, which is no fault: no line.
        let at_the_edge = |feed| proto::GetStatusResponse {
            feed,
            ..periphery()
        };
        assert_eq!(feed(&at_the_edge(Some(waiting))), None);
        assert_eq!(feed(&at_the_edge(None)), None);
    }

    // spec/areas/hubs.md#the-validator-in-the-node: a hub that runs reads
    // as one, with a hint only where something is wrong.
    #[test]
    fn a_validated_hub_gets_a_hint_only_when_something_is_wrong() {
        let now = 1_800_000_000_000;
        let running = proto::HubValidator {
            hub: "ab".repeat(32),
            index: 1,
            validators: 4,
            height: 1234,
            final_unix_ms: now - 100,
            view: 1300,
            phase: proto::HubPhase::Running.into(),
            stalled: false,
            escrow_checked: true,
            xrpl_connected: true,
        };
        assert_eq!(
            validating(&running, now),
            "hub abababababababab…, validator 2 of 4, block 1234 in view 1300"
        );
        let quiet = proto::HubValidator {
            final_unix_ms: now - 5 * 60_000,
            escrow_checked: false,
            xrpl_connected: false,
            ..running.clone()
        };
        let text = validating(&quiet, now);
        assert!(text.contains("no block final for 5 minutes"), "{text}");
        assert!(text.contains("no deposit is credited"), "{text}");
        assert!(text.contains("XRPL server cannot be reached"), "{text}");
        let stalled = proto::HubValidator {
            stalled: true,
            ..quiet
        };
        let text = validating(&stalled, now);
        assert!(text.contains("stalled"), "{text}");
        assert!(!text.contains("no block final"), "{text}");
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
