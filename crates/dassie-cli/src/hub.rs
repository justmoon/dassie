//! `dassie hub`: the node's account in each settlement hub it is in, and
//! the owner's moves between it and the owner book of the hub's asset (on
//! the node's XRP Ledger account on the hub's network: XRP for an XRP hub,
//! DSUSD or RLUSD for a token hub), over the IPC socket
//! (spec/areas/hubs.md#in-the-node).
//! Amounts are in the hub's asset, which its scheme names.
//!
//! - `dassie hub status` shows each hub's account, its holds, the floor and
//!   the ceiling the node keeps it between, and the move on its way.
//! - `dassie hub deposit <amount>` and `dassie hub withdraw <amount>` move
//!   funds now, which pauses the node's own moves of that hub for a while.
//!   A token hub's withdrawal may first have the node check, and top up,
//!   the XRP its escrow pays fees with: try again shortly.
//! - `dassie hub auto on|off` turns the node's own moves on or off.
//! - `dassie hub give-up` gives up a move that is late; the node's own
//!   moves of that hub then wait for the owner.
//! - `dassie hub found`, `foundings`, `drop` and `payout` found hubs, and
//!   show the payout bundles kept ([`crate::founding`]).
//!
//! Each of `deposit`, `withdraw`, `auto` and `give-up` acts on one hub:
//! `--hub <ID>` names it, as `status` shows it, and is needed only while
//! the node is in more than one.

use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};
use lexopt::prelude::*;

use crate::client::Client;
use crate::status::{amount, date, entries, line, unexpected};

/// The scale the hub ledgers count their asset at, XRP and dollars alike:
/// amounts are in their units.
const SCALE: u32 = 9;

/// What `dassie hub` is asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show each hub's account.
    Status,
    /// Act on the hub named by its ID in hex, or the node's only hub.
    Move(Move, String),
    /// Found a hub, or see the foundings or the payout bundles.
    Founding(crate::founding::Command),
}

/// What the owner does with one hub's account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Deposit this many units from the owner book of the hub's asset.
    Deposit(u64),
    /// Withdraw this many units to the XRP Ledger account.
    Withdraw(u64),
    /// Turn the node's own moves on or off.
    Automatic(bool),
    /// Give up the late move.
    GiveUp,
}

/// Reads `dassie hub`'s arguments.
///
/// # Errors
///
/// A command line that is not one of the above.
pub fn args(parser: &mut lexopt::Parser) -> Result<Command, lexopt::Error> {
    let action = match parser.next()? {
        None => return Ok(Command::Status),
        Some(Value(word)) => match word.string()?.as_str() {
            "status" => None,
            "deposit" => Some(Move::Deposit(hub_amount(&parser.value()?.string()?)?)),
            "withdraw" => Some(Move::Withdraw(hub_amount(&parser.value()?.string()?)?)),
            "auto" => match parser.value()?.string()?.as_str() {
                "on" => Some(Move::Automatic(true)),
                "off" => Some(Move::Automatic(false)),
                other => return Err(format!("`auto` takes on or off, not {other:?}").into()),
            },
            "give-up" => Some(Move::GiveUp),
            word @ ("found" | "foundings" | "drop" | "payout") => {
                return crate::founding::args(word, parser).map(Command::Founding);
            }
            other => return Err(format!("unknown hub command {other:?}").into()),
        },
        Some(other) => return Err(other.unexpected()),
    };
    let mut hub = String::new();
    while let Some(arg) = parser.next()? {
        match arg {
            Long("hub") if action.is_some() && hub.is_empty() => {
                hub = hub_id(&parser.value()?.string()?)?;
            }
            other => return Err(other.unexpected()),
        }
    }
    Ok(action.map_or(Command::Status, |action| Command::Move(action, hub)))
}

/// A hub's ID, as `dassie hub status` shows it: 64 hex digits.
fn hub_id(text: &str) -> Result<String, String> {
    let hex = text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !hex {
        return Err(format!(
            "{text:?} is not a hub's ID, the 64 hex digits `dassie hub status` shows"
        ));
    }
    Ok(text.to_ascii_lowercase())
}

/// An amount of XRP, such as `2.5`, in the hub ledger's units.
pub(crate) fn xrp(text: &str) -> Result<u64, String> {
    units(text, "XRP")
}

/// An amount of a hub's asset, such as `2.5`, in its ledger's units, which
/// count XRP and dollars at one scale.
fn hub_amount(text: &str) -> Result<u64, String> {
    units(text, "the hub's asset")
}

/// An amount of `what`, such as `2.5`, in units at [`SCALE`].
fn units(text: &str, what: &str) -> Result<u64, String> {
    let invalid = || format!("{text:?} is not an amount of {what}, such as 2.5");
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let scale = usize::try_from(SCALE).unwrap_or(usize::MAX);
    if whole.is_empty() || !digits(whole) || !digits(fraction) || fraction.len() > scale {
        return Err(invalid());
    }
    let whole: u64 = whole.parse().map_err(|_| invalid())?;
    let padded = format!("{fraction:0<scale$}");
    let fraction: u64 = padded.parse().map_err(|_| invalid())?;
    whole
        .checked_mul(10u64.pow(SCALE))
        .and_then(|units| units.checked_add(fraction))
        .filter(|units| *units > 0)
        .ok_or_else(invalid)
}

/// Runs `dassie hub`.
pub async fn run(env: &Env, command: Command) -> Result<(), String> {
    if let Command::Founding(command) = command {
        return crate::founding::run(env, command).await;
    }
    let mut client = Client::connect(env).await.map_err(|error| error.message)?;
    let Command::Move(action, hub_id) = command else {
        let hubs = statuses(&mut client).await?;
        if hubs.is_empty() {
            println!("The node is in no settlement hub: no hub scheme runs.");
        }
        for (index, status) in hubs.iter().enumerate() {
            if index > 0 {
                println!();
            }
            show(status, now_ms());
        }
        return Ok(());
    };
    let method = match action {
        Move::Deposit(amount) => Method::HubDeposit(proto::HubDepositRequest { amount, hub_id }),
        Move::Withdraw(amount) => Method::HubWithdraw(proto::HubWithdrawRequest { amount, hub_id }),
        Move::Automatic(automatic) => {
            Method::SetHubAutomatic(proto::SetHubAutomaticRequest { automatic, hub_id })
        }
        Move::GiveUp => Method::HubGiveUp(proto::HubGiveUpRequest { hub_id }),
    };
    let status = match client.call(method).await {
        Ok(
            response::Method::HubDeposit(proto::HubDepositResponse { status })
            | response::Method::HubWithdraw(proto::HubWithdrawResponse { status })
            | response::Method::SetHubAutomatic(proto::SetHubAutomaticResponse { status })
            | response::Method::HubGiveUp(proto::HubGiveUpResponse { status }),
        ) => status.unwrap_or_default(),
        other => return Err(unexpected(other)),
    };
    show(&status, now_ms());
    Ok(())
}

/// Each hub the node is in whose scheme runs.
async fn statuses(client: &mut Client) -> Result<Vec<proto::HubStatus>, String> {
    match client
        .call(Method::GetHubStatus(proto::GetHubStatusRequest {}))
        .await
    {
        Ok(response::Method::GetHubStatus(proto::GetHubStatusResponse { hubs })) => Ok(hubs),
        other => Err(unexpected(other)),
    }
}

/// Each hub's line in `dassie status`, for each hub scheme that runs.
pub async fn summary(client: &mut Client) -> Vec<String> {
    statuses(client)
        .await
        .unwrap_or_default()
        .iter()
        .map(|status| format!("{}: {}", status.scheme, brief(status)))
        .collect()
}

/// The account in a few words: its number and balance, how it stands, and
/// how many validators answer.
fn brief(status: &proto::HubStatus) -> String {
    let account = match status.account {
        Some(number) => format!("account {number}, {}", text(status, status.balance)),
        None => "no account yet".into(),
    };
    format!(
        "{account}, {}; {} of {} validators answering",
        standing(status),
        status.validators_answering,
        status.validators
    )
}

fn standing(status: &proto::HubStatus) -> &'static str {
    // A frozen hub's account follows it all the same, but is not in step
    // for moves or packets (spec/areas/hubs.md#a-frozen-hub). One it
    // cannot follow says so all the same.
    let failed = status.connection == i32::from(proto::LedgerConnection::Failed);
    if status.frozen_since_unix_ms.is_some() && !failed {
        return "frozen by its token's issuer";
    }
    match proto::LedgerConnection::try_from(status.connection) {
        Ok(proto::LedgerConnection::Synced) => "in sync",
        Ok(proto::LedgerConnection::Connecting) => "connecting",
        Ok(proto::LedgerConnection::Syncing) => "catching up",
        Ok(proto::LedgerConnection::Unfunded) => "waiting for the first deposit to open it",
        Ok(proto::LedgerConnection::Failed) => "failed",
        _ => "unknown",
    }
}

fn signed(units: u64) -> i64 {
    i64::try_from(units).unwrap_or(i64::MAX)
}

/// The code of the asset of the hub of `status`, as its scheme names it
/// last (spec/decisions/hub-scheme-names.md): `XRP`, `DSUSD`.
fn code(status: &proto::HubStatus) -> String {
    status
        .scheme
        .rsplit('-')
        .next()
        .filter(|code| !code.is_empty() && status.scheme.starts_with("hub-"))
        .map_or_else(|| "XRP".into(), str::to_ascii_uppercase)
}

/// `units` of the asset of the hub of `status`.
fn text(status: &proto::HubStatus, units: u64) -> String {
    format!("{} {}", amount(signed(units), SCALE), code(status))
}

/// Prints a hub's account, line by line.
fn show(status: &proto::HubStatus, now_ms: i64) {
    println!("Settlement hub {}", status.hub_id);
    line("Scheme", &status.scheme);
    line("Account", &brief(status));
    line(
        "Balance",
        &format!(
            "{} spendable, {} reserve",
            text(status, status.balance),
            text(status, status.reserve)
        ),
    );
    line(
        "Moved",
        &format!(
            "{} deposited, {} withdrawn, {} came back",
            text(status, status.deposited),
            text(status, status.withdrawn),
            text(status, status.returned)
        ),
    );
    if let Some(held) = issuer_text(status) {
        line("Issuer", &held);
    }
    line("Keeps", &keeps(status, now_ms));
    if let Some(moving) = &status.moving {
        line("Moving", &moving_text(status, moving));
    }
    if let Some(given_up) = &status.given_up {
        line("Given up", &given_up_text(status, given_up));
    }
    line("Paying", &holds(status, &status.paying, "to"));
    line("Taking", &holds(status, &status.taking, "from"));
}

/// What the issuer of a token hub's asset holds of its escrow, if it holds
/// it: a freeze, and since when, or a hold that keeps an ended hub's payout
/// back (spec/areas/hubs.md#a-frozen-hub).
fn issuer_text(status: &proto::HubStatus) -> Option<String> {
    match (status.frozen_since_unix_ms, status.escrow_held) {
        (Some(since), _) => Some(format!(
            "frozen since {}: no packet goes over the hub, and nothing moves into or out of \
             it until the issuer lets go; claims the hub still pays go, and what waits goes \
             after the thaw",
            date(since)
        )),
        (None, true) => {
            Some("holds the escrow: it pays out nothing until the issuer lets go".into())
        }
        (None, false) => None,
    }
}

/// The floor, the ceiling, and whether the node moves funds by itself.
fn keeps(status: &proto::HubStatus, now_ms: i64) -> String {
    let who = match (status.automatic, status.paused_until_unix_ms) {
        (false, _) => "the node's own moves are off (`dassie hub auto on`)".into(),
        (true, _) if status.given_up.is_some() => {
            "the node's own moves wait for you after a move you gave up (`dassie hub auto on`)"
                .into()
        }
        (true, Some(until)) if until > now_ms => {
            format!("the node's own moves wait until {}", date(until))
        }
        (true, _) => "the node moves funds by itself".into(),
    };
    format!(
        "between {} and {}; {who}",
        text(status, status.floor),
        text(status, status.ceiling)
    )
}

/// A move the owner gave up: what the other side may still show.
fn given_up_text(status: &proto::HubStatus, given_up: &proto::HubMove) -> String {
    match proto::hub_move::Direction::try_from(given_up.direction) {
        Ok(proto::hub_move::Direction::Withdraw) => format!(
            "a withdrawal of {}; the XRP Ledger account may still receive it",
            text(status, given_up.amount)
        ),
        _ => format!(
            "a deposit of {}; the hub may still credit it",
            text(status, given_up.amount)
        ),
    }
}

fn moving_text(status: &proto::HubStatus, moving: &proto::HubMove) -> String {
    let direction = match proto::hub_move::Direction::try_from(moving.direction) {
        Ok(proto::hub_move::Direction::Withdraw) => "a withdrawal",
        _ => "a deposit",
    };
    let whose = if moving.by_owner {
        "yours"
    } else {
        "the node's"
    };
    let mut said = format!("{direction} of {}, {whose}", text(status, moving.amount));
    if moving.late {
        said.push_str(
            ". IT IS LATE: the hub has not shown it yet; the node waits for it and pays \
             nothing again",
        );
    }
    said
}

fn holds(status: &proto::HubStatus, holds: &[proto::HubHold], way: &str) -> String {
    if holds.is_empty() {
        return "no holds".into();
    }
    entries(holds.iter().map(|hold| {
        let who = if hold.peer.is_empty() {
            format!("account {}", hold.account)
        } else {
            format!("{} (account {})", hold.peer, hold.account)
        };
        format!("{} {way} {who}", text(status, hold.amount))
    }))
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
    fn amounts_of_xrp_are_read_exactly() {
        assert_eq!(xrp("2.5"), Ok(2_500_000_000));
        assert_eq!(xrp("1"), Ok(1_000_000_000));
        assert_eq!(xrp("0.000000001"), Ok(1));
        for wrong in ["", ".5", "1.0000000001", "-1", "1e3", "0", "x"] {
            assert!(xrp(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_command_line_names_one_move() {
        let parse = |args: &[&str]| args_of(args).map_err(|error| error.to_string());
        let only = |action| Ok(Command::Move(action, String::new()));
        assert_eq!(parse(&[]), Ok(Command::Status));
        assert_eq!(parse(&["status"]), Ok(Command::Status));
        assert_eq!(parse(&["deposit", "3"]), only(Move::Deposit(3_000_000_000)));
        assert_eq!(
            parse(&["withdraw", "0.5"]),
            only(Move::Withdraw(500_000_000))
        );
        assert_eq!(parse(&["auto", "off"]), only(Move::Automatic(false)));
        assert!(parse(&["auto", "maybe"]).is_err());
        assert_eq!(parse(&["give-up"]), only(Move::GiveUp));
        assert!(parse(&["deposit"]).is_err());
        assert!(parse(&["status", "now"]).is_err());
    }

    // A move names its hub only while the node is in several; status shows
    // them all.
    #[test]
    fn a_move_names_its_hub_by_its_id() {
        let parse = |args: &[&str]| args_of(args).map_err(|error| error.to_string());
        let id = "AB".repeat(32);
        assert_eq!(
            parse(&["withdraw", "1", "--hub", &id]),
            Ok(Command::Move(
                Move::Withdraw(1_000_000_000),
                "ab".repeat(32)
            ))
        );
        assert_eq!(
            parse(&["give-up", "--hub", &id]),
            Ok(Command::Move(Move::GiveUp, "ab".repeat(32)))
        );
        assert!(parse(&["give-up", "--hub", "abc"]).is_err());
        assert!(parse(&["give-up", "--hub", &id, "--hub", &id]).is_err());
        assert!(parse(&["status", "--hub", &id]).is_err());
        assert!(parse(&["deposit", "--hub", &id]).is_err());
    }

    fn args_of(words: &[&str]) -> Result<Command, lexopt::Error> {
        let mut parser = lexopt::Parser::from_args(words.iter().copied());
        args(&mut parser)
    }

    #[test]
    fn the_status_says_where_the_account_stands() {
        let mut status = proto::HubStatus {
            account: Some(5),
            connection: proto::LedgerConnection::Synced.into(),
            balance: 5_200_000_000,
            validators: 4,
            validators_answering: 3,
            automatic: true,
            floor: 400_000_000,
            ceiling: 10_000_000_000,
            ..Default::default()
        };
        assert_eq!(
            brief(&status),
            "account 5, 5.2 XRP, in sync; 3 of 4 validators answering"
        );
        assert_eq!(
            keeps(&status, 0),
            "between 0.4 XRP and 10 XRP; the node moves funds by itself"
        );
        status.paused_until_unix_ms = Some(60_000);
        assert!(keeps(&status, 0).contains("wait until 1970-01-01 00:01 UTC"));
        status.account = None;
        status.connection = proto::LedgerConnection::Unfunded.into();
        assert!(brief(&status).starts_with("no account yet, waiting for the first deposit"));
        let moving = proto::HubMove {
            direction: proto::hub_move::Direction::Withdraw.into(),
            amount: 1_000_000_000,
            by_owner: true,
            ..Default::default()
        };
        assert_eq!(
            moving_text(&status, &moving),
            "a withdrawal of 1 XRP, yours"
        );
        let late = proto::HubMove {
            late: true,
            ..moving
        };
        assert!(moving_text(&status, &late).contains("IT IS LATE"));
    }

    // spec/areas/hubs.md#a-frozen-hub: a frozen hub says so, and since
    // when; an XRP hub's issuer never holds anything.
    #[test]
    fn a_frozen_hub_says_so() {
        let mut status = proto::HubStatus {
            scheme: "hub-xrpl-testnet-dsusd".into(),
            account: Some(2),
            connection: proto::LedgerConnection::Syncing.into(),
            validators: 4,
            validators_answering: 4,
            ..Default::default()
        };
        assert_eq!(issuer_text(&status), None);
        status.frozen_since_unix_ms = Some(60_000);
        status.escrow_held = true;
        assert!(brief(&status).contains("frozen by its token's issuer"));
        let frozen = issuer_text(&status).unwrap();
        assert!(frozen.starts_with("frozen since 1970-01-01 00:01 UTC"));
        assert!(!frozen.contains("  "), "{frozen}");
        status.connection = proto::LedgerConnection::Failed.into();
        assert!(brief(&status).contains("failed"));
        status.connection = proto::LedgerConnection::Syncing.into();
        status.frozen_since_unix_ms = None;
        assert!(issuer_text(&status).is_some_and(|text| text.contains("pays out nothing")));
        assert!(brief(&status).contains("catching up"));
    }

    // A hub's amounts are in its asset, which its scheme names last.
    #[test]
    fn amounts_are_in_the_hubs_asset() {
        let mut status = proto::HubStatus {
            scheme: "hub-xrpl-testnet-dsusd".into(),
            account: Some(2),
            balance: 1_500_000_000,
            ..Default::default()
        };
        assert!(brief(&status).starts_with("account 2, 1.5 DSUSD"));
        status.scheme = "hub-xrpl-testnet-xrp".into();
        assert!(brief(&status).starts_with("account 2, 1.5 XRP"));
        assert_eq!(hub_amount("2.5"), Ok(2_500_000_000));
        assert!(hub_amount("x").is_err_and(|error| error.contains("the hub's asset")));
    }
}
