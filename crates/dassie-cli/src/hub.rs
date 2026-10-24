//! `dassie hub`: the node's account in its settlement hub, and the owner's
//! moves between it and the node's XRP Ledger testnet account, over the IPC
//! socket (spec/areas/hubs.md#in-the-node).
//!
//! - `dassie hub status` shows the account, its holds, the floor and the
//!   ceiling the node keeps it between, and the move on its way.
//! - `dassie hub deposit <XRP>` and `dassie hub withdraw <XRP>` move funds
//!   now, which pauses the node's own moves for a while.
//! - `dassie hub auto on|off` turns the node's own moves on or off.
//! - `dassie hub give-up` gives up a move that is late; the node's own
//!   moves then wait for the owner.

use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};
use lexopt::prelude::*;

use crate::client::Client;
use crate::status::{amount, date, entries, line, unexpected};

/// The scale the hub ledger counts XRP at: amounts are in its units.
const SCALE: u32 = 9;

/// What `dassie hub` is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the account.
    Status,
    /// Deposit this many units from the XRP Ledger account.
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
    let command = match parser.next()? {
        None => Command::Status,
        Some(Value(word)) => match word.string()?.as_str() {
            "status" => Command::Status,
            "deposit" => Command::Deposit(xrp(&parser.value()?.string()?)?),
            "withdraw" => Command::Withdraw(xrp(&parser.value()?.string()?)?),
            "auto" => match parser.value()?.string()?.as_str() {
                "on" => Command::Automatic(true),
                "off" => Command::Automatic(false),
                other => return Err(format!("`auto` takes on or off, not {other:?}").into()),
            },
            "give-up" => Command::GiveUp,
            other => return Err(format!("unknown hub command {other:?}").into()),
        },
        Some(other) => return Err(other.unexpected()),
    };
    if let Some(extra) = parser.next()? {
        return Err(extra.unexpected());
    }
    Ok(command)
}

/// An amount of XRP, such as `2.5`, in the hub ledger's units.
fn xrp(text: &str) -> Result<u64, String> {
    let invalid = || format!("{text:?} is not an amount of XRP, such as 2.5");
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
    let mut client = Client::connect(env).await.map_err(|error| error.message)?;
    let method = match command {
        Command::Status => Method::GetHubStatus(proto::GetHubStatusRequest {}),
        Command::Deposit(amount) => Method::HubDeposit(proto::HubDepositRequest { amount }),
        Command::Withdraw(amount) => Method::HubWithdraw(proto::HubWithdrawRequest { amount }),
        Command::Automatic(automatic) => {
            Method::SetHubAutomatic(proto::SetHubAutomaticRequest { automatic })
        }
        Command::GiveUp => Method::HubGiveUp(proto::HubGiveUpRequest {}),
    };
    let status = match client.call(method).await {
        Ok(
            response::Method::GetHubStatus(proto::GetHubStatusResponse { status })
            | response::Method::HubDeposit(proto::HubDepositResponse { status })
            | response::Method::HubWithdraw(proto::HubWithdrawResponse { status })
            | response::Method::SetHubAutomatic(proto::SetHubAutomaticResponse { status })
            | response::Method::HubGiveUp(proto::HubGiveUpResponse { status }),
        ) => status.unwrap_or_default(),
        other => return Err(unexpected(other)),
    };
    show(&status, now_ms());
    Ok(())
}

/// The hub's line in `dassie status`, while a hub scheme runs.
pub async fn summary(client: &mut Client) -> Option<String> {
    match client
        .call(Method::GetHubStatus(proto::GetHubStatusRequest {}))
        .await
    {
        Ok(response::Method::GetHubStatus(proto::GetHubStatusResponse {
            status: Some(status),
        })) => Some(brief(&status)),
        _ => None,
    }
}

/// The account in a few words: its number and balance, how it stands, and
/// how many validators answer.
fn brief(status: &proto::HubStatus) -> String {
    let account = match status.account {
        Some(number) => format!(
            "account {number}, {} XRP",
            amount(signed(status.balance), SCALE)
        ),
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

fn xrp_text(units: u64) -> String {
    format!("{} XRP", amount(signed(units), SCALE))
}

/// Prints the account, line by line.
fn show(status: &proto::HubStatus, now_ms: i64) {
    println!("Settlement hub {}", status.hub_id);
    line("Account", &brief(status));
    line(
        "Balance",
        &format!(
            "{} spendable, {} reserve",
            xrp_text(status.balance),
            xrp_text(status.reserve)
        ),
    );
    line(
        "Moved",
        &format!(
            "{} deposited, {} withdrawn, {} came back",
            xrp_text(status.deposited),
            xrp_text(status.withdrawn),
            xrp_text(status.returned)
        ),
    );
    line("Keeps", &keeps(status, now_ms));
    if let Some(moving) = &status.moving {
        line("Moving", &moving_text(moving));
    }
    if let Some(given_up) = &status.given_up {
        line("Given up", &given_up_text(given_up));
    }
    line("Paying", &holds(&status.paying, "to"));
    line("Taking", &holds(&status.taking, "from"));
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
        xrp_text(status.floor),
        xrp_text(status.ceiling)
    )
}

/// A move the owner gave up: what the other side may still show.
fn given_up_text(given_up: &proto::HubMove) -> String {
    match proto::hub_move::Direction::try_from(given_up.direction) {
        Ok(proto::hub_move::Direction::Withdraw) => format!(
            "a withdrawal of {}; the XRP Ledger account may still receive it",
            xrp_text(given_up.amount)
        ),
        _ => format!(
            "a deposit of {}; the hub may still credit it",
            xrp_text(given_up.amount)
        ),
    }
}

fn moving_text(moving: &proto::HubMove) -> String {
    let direction = match proto::hub_move::Direction::try_from(moving.direction) {
        Ok(proto::hub_move::Direction::Withdraw) => "a withdrawal",
        _ => "a deposit",
    };
    let whose = if moving.by_owner {
        "yours"
    } else {
        "the node's"
    };
    let mut text = format!("{direction} of {}, {whose}", xrp_text(moving.amount));
    if moving.late {
        text.push_str(
            ". IT IS LATE: the hub has not shown it yet; the node waits for it and pays \
             nothing again",
        );
    }
    text
}

fn holds(holds: &[proto::HubHold], way: &str) -> String {
    if holds.is_empty() {
        return "no holds".into();
    }
    entries(holds.iter().map(|hold| {
        let who = if hold.peer.is_empty() {
            format!("account {}", hold.account)
        } else {
            format!("{} (account {})", hold.peer, hold.account)
        };
        format!("{} {way} {who}", xrp_text(hold.amount))
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
        assert_eq!(parse(&[]), Ok(Command::Status));
        assert_eq!(
            parse(&["deposit", "3"]),
            Ok(Command::Deposit(3_000_000_000))
        );
        assert_eq!(
            parse(&["withdraw", "0.5"]),
            Ok(Command::Withdraw(500_000_000))
        );
        assert_eq!(parse(&["auto", "off"]), Ok(Command::Automatic(false)));
        assert!(parse(&["auto", "maybe"]).is_err());
        assert_eq!(parse(&["give-up"]), Ok(Command::GiveUp));
        assert!(parse(&["deposit"]).is_err());
        assert!(parse(&["status", "now"]).is_err());
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
        assert_eq!(moving_text(&moving), "a withdrawal of 1 XRP, yours");
        let late = proto::HubMove {
            late: true,
            ..moving
        };
        assert!(moving_text(&late).contains("IT IS LATE"));
    }
}
