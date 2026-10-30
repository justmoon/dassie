//! `dassie hub`: the node's account in each settlement hub it is in, and
//! the owner's moves between it and the owner book of the hub's asset (on
//! the node's XRP Ledger testnet account: XRP for the XRP hub, DSUSD for
//! the DSUSD hub), over the IPC socket (spec/areas/hubs.md#in-the-node).
//! Amounts are in the hub's asset, which its scheme names.
//!
//! - `dassie hub status` shows each hub's account, its holds, the target it
//!   keeps the account near, the floor below which it deposits, the
//!   ceiling it withdraws beyond to the owner's own account, the move on
//!   its way, and how many peerings over the hub its pool covers, with the
//!   newcomers the node refuses while it does not cover one more
//!   (spec/decisions/slots-by-liquidity.md).
//! - `dassie hub deposit <scheme> <amount>` and `dassie hub withdraw
//!   <scheme> <amount>` move funds now, which pauses the node's own moves
//!   of that hub for a while. A token hub's withdrawal may first have the
//!   node check, and top up, the XRP its escrow pays fees with: try again
//!   shortly.
//! - `dassie hub auto <scheme> on|off` turns the node's own moves on or
//!   off.
//! - `dassie hub target <scheme> <amount>|default` sets the target the
//!   node keeps the hub near, and with it the floor and the ceiling, a
//!   tenth either side, or goes back to the default.
//! - `dassie hub give-up <scheme>` gives up a move that is late; the
//!   node's own moves of that hub then wait for the owner.
//! - `dassie hub leave <scheme>` leaves a hub that ended, once a quorum of
//!   its validators signed its payout set, so the scheme can be added
//!   again with another hub's rules
//!   (spec/areas/hubs.md#leaving-a-hub-that-ended).
//! - `dassie hub found`, `foundings`, `drop` and `payout` found hubs, and
//!   show the payout bundles kept ([`crate::founding`]).
//!
//! Each of `deposit`, `withdraw`, `auto`, `target`, `give-up`, `leave` and
//! `payout` acts on
//! one hub, which it names first by the settlement scheme over it, as
//! `status` shows it, such as `hub-xrpl-testnet-xrp`: always, so that a
//! script means the same hub once the node is in more than one.

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
    /// Act on the hub over this scheme.
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
    /// Keep the hub near this many units, or near the default.
    Target(Option<u64>),
    /// Give up the late move.
    GiveUp,
    /// Leave the hub, which ended.
    Leave,
}

/// Reads `dassie hub`'s arguments.
///
/// # Errors
///
/// A command line that is not one of the above.
pub fn args(parser: &mut lexopt::Parser) -> Result<Command, lexopt::Error> {
    let command = match parser.next()? {
        None => return Ok(Command::Status),
        Some(Value(word)) => match word.string()?.as_str() {
            "status" => Command::Status,
            word @ ("deposit" | "withdraw" | "auto" | "target" | "give-up" | "leave") => {
                let scheme = hub_scheme(&parser.value()?.string()?)?;
                let action = match word {
                    "deposit" => Move::Deposit(hub_amount(&parser.value()?.string()?)?),
                    "withdraw" => Move::Withdraw(hub_amount(&parser.value()?.string()?)?),
                    "auto" => match parser.value()?.string()?.as_str() {
                        "on" => Move::Automatic(true),
                        "off" => Move::Automatic(false),
                        other => {
                            return Err(format!("`auto` takes on or off, not {other:?}").into());
                        }
                    },
                    "target" => match parser.value()?.string()?.as_str() {
                        "default" => Move::Target(None),
                        amount => Move::Target(Some(hub_amount(amount)?)),
                    },
                    "leave" => Move::Leave,
                    _ => Move::GiveUp,
                };
                Command::Move(action, scheme)
            }
            word @ ("found" | "foundings" | "drop" | "payout") => {
                return crate::founding::args(word, parser).map(Command::Founding);
            }
            other => return Err(format!("unknown hub command {other:?}").into()),
        },
        Some(other) => return Err(other.unexpected()),
    };
    if let Some(extra) = parser.next()? {
        return Err(extra.unexpected());
    }
    Ok(command)
}

/// The settlement scheme over a hub, as `dassie hub status` shows it, such
/// as `hub-xrpl-testnet-xrp`: whether the node runs it is the node's to
/// say.
pub(crate) fn hub_scheme(text: &str) -> Result<String, String> {
    if !text.starts_with("hub-") {
        return Err(format!(
            "{text:?} is not the scheme over a hub, such as hub-xrpl-testnet-xrp, as \
             `dassie hub status` shows it"
        ));
    }
    Ok(text.to_owned())
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
    let Command::Move(action, scheme) = command else {
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
        Move::Deposit(amount) => Method::HubDeposit(proto::HubDepositRequest { amount, scheme }),
        Move::Withdraw(amount) => Method::HubWithdraw(proto::HubWithdrawRequest { amount, scheme }),
        Move::Automatic(automatic) => {
            Method::SetHubAutomatic(proto::SetHubAutomaticRequest { automatic, scheme })
        }
        Move::Target(target) => Method::SetHubTarget(proto::SetHubTargetRequest { scheme, target }),
        Move::GiveUp => Method::HubGiveUp(proto::HubGiveUpRequest { scheme }),
        Move::Leave => return leave(&mut client, scheme).await,
    };
    let status = match client.call(method).await {
        Ok(
            response::Method::HubDeposit(proto::HubDepositResponse { status })
            | response::Method::HubWithdraw(proto::HubWithdrawResponse { status })
            | response::Method::SetHubAutomatic(proto::SetHubAutomaticResponse { status })
            | response::Method::SetHubTarget(proto::SetHubTargetResponse { status })
            | response::Method::HubGiveUp(proto::HubGiveUpResponse { status }),
        ) => status.unwrap_or_default(),
        other => return Err(unexpected(other)),
    };
    show(&status, now_ms());
    Ok(())
}

/// Leaves the hub over `scheme`, which ended, and says what follows.
async fn leave(client: &mut Client, scheme: String) -> Result<(), String> {
    let request = Method::LeaveHub(proto::LeaveHubRequest {
        scheme: scheme.clone(),
    });
    let gone = match client.call(request).await {
        Ok(response::Method::LeaveHub(proto::LeaveHubResponse { gone })) => gone,
        other => return Err(unexpected(other)),
    };
    println!("{}", left_text(&scheme, gone));
    Ok(())
}

/// What leaving the hub over `scheme` did, and what the owner does next.
fn left_text(scheme: &str, gone: bool) -> String {
    let when = if gone {
        "The node left the hub"
    } else {
        "The node leaves the hub once its peerings' methods over it are wound down \
         (`dassie ledger` shows the scheme being removed until then)"
    };
    format!(
        "{when}: what its books held of the account there is written off, and the hub's \
         payout set pays it to your own XRP Ledger account, where it is booked as it \
         arrives. Join the default hub of its asset again on the wallet's Assets page, or \
         with `dassie setup --hub {scheme}` on a node set up from its phrase."
    )
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
    let short = status
        .liquidity
        .as_ref()
        .and_then(refusing)
        .map(|refusing| format!("; {refusing}"))
        .unwrap_or_default();
    format!(
        "{account}, {}; {} of {} validators answering{short}",
        standing(status),
        status.validators_answering,
        status.validators
    )
}

/// The newcomers the node refuses over a hub for want of pool, and why, if
/// it refuses any (spec/decisions/slots-by-liquidity.md).
fn refusing(liquidity: &proto::HubLiquidity) -> Option<String> {
    let whom = match (liquidity.refusing_core_peers, liquidity.refusing_downlinks) {
        (true, _) => "no new peers",
        (false, true) => "no new downlinks",
        (false, false) => return None,
    };
    let covers = format!(
        "its pool covers {} of its {} peerings at a largest packet each",
        liquidity.covered, liquidity.peerings
    );
    let why = if liquidity.short_since_unix_ms.is_some() {
        covers
    } else {
        format!("{covers}, and no more")
    };
    Some(format!("takes {whom}: {why}"))
}

/// The pool against the peerings over the hub, in full: what it covers, the
/// newcomers it refuses, and when the node ends peerings if it stays short.
fn liquidity_text(status: &proto::HubStatus, liquidity: &proto::HubLiquidity) -> String {
    let Some(refusing) = refusing(liquidity) else {
        return format!(
            "{} covers its {} peerings at a largest packet each, and more; takes new peers",
            text(status, liquidity.pool),
            liquidity.peerings
        );
    };
    let ends = liquidity
        .ends_from_unix_ms
        .filter(|_| liquidity.short_since_unix_ms.is_some())
        .map(|from| {
            format!(
                "; if it is still short at {}, the node ends as many peerings as bring it back to a \
                 packet each, downlinks first",
                date(from)
            )
        })
        .unwrap_or_default();
    format!("{}: {refusing}{ends}", text(status, liquidity.pool))
}

fn standing(status: &proto::HubStatus) -> &'static str {
    // A frozen hub's account follows it all the same, but is not in step
    // for moves or packets (spec/areas/hubs.md#a-frozen-hub). One it
    // cannot follow says so all the same.
    let failed = status.connection == i32::from(proto::LedgerConnection::Failed);
    if status.ended && !failed {
        return "the hub ended: its payout set is signed, and you may leave it";
    }
    if status.stopped && !failed {
        return "the hub stalled or is shutting down: it takes nothing more";
    }
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
    println!("Settlement hub {}", status.scheme);
    line("ID", &status.hub_id);
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
    if let Some(ended) = ended_text(status) {
        line("Ended", &ended);
    }
    line("Keeps", &keeps(status, now_ms));
    if let Some(moving) = &status.moving {
        line("Moving", &moving_text(status, moving));
    }
    if let Some(given_up) = &status.given_up {
        line("Given up", &given_up_text(status, given_up));
    }
    if let Some(liquidity) = &status.liquidity {
        line("Pool", &liquidity_text(status, liquidity));
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

/// What the owner can do about a hub that stopped, if it did
/// (spec/areas/hubs.md#leaving-a-hub-that-ended).
fn ended_text(status: &proto::HubStatus) -> Option<String> {
    if status.ended {
        Some(format!(
            "a quorum of its validators signed its payout set (`dassie hub payout {0}`), which \
             anyone may submit; leave the hub with `dassie hub leave {0}` to join another, \
             once you copied the payout: after you leave, `dassie hub payout` no longer shows it",
            status.scheme
        ))
    } else if status.stopped {
        Some(
            "the hub stalled or is shutting down: nothing moves into or out of it, and it can be \
             left once a quorum of its validators signed its payout set"
                .into(),
        )
    } else {
        None
    }
}

/// The target, the owner's or the default, the floor and the ceiling where
/// the node withdraws beyond it, and whether the node moves funds by
/// itself.
fn keeps(status: &proto::HubStatus, now_ms: i64) -> String {
    let who = match (status.automatic, status.paused_until_unix_ms) {
        (false, _) => format!(
            "the node's own moves are off (`dassie hub auto {} on`)",
            status.scheme
        ),
        (true, _) if status.given_up.is_some() => format!(
            "the node's own moves wait for you after a move you gave up (`dassie hub auto {} \
             on`)",
            status.scheme
        ),
        (true, Some(until)) if until > now_ms => {
            format!("the node's own moves wait until {}", date(until))
        }
        (true, _) => "the node moves funds by itself".into(),
    };
    let whose = if status.target_by_owner {
        "your target".to_owned()
    } else {
        format!(
            "the default (`dassie hub target {} <amount>` sets yours)",
            status.scheme
        )
    };
    // spec/decisions/one-balance-per-asset.md: the hub keeps a float, and
    // what is beyond it goes to the owner's own account.
    format!(
        "near {}, {whose}: between {} and {}, what is beyond going to your own account; {who}",
        text(status, status.target),
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

/// Each hold, and for one of ours toward a peer, the peer's allotment: its
/// share of what the node can hold in the hub
/// (spec/decisions/hold-allotment.md).
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
        let mut said = format!("{} {way} {who}", text(status, hold.amount));
        if let Some(allotment) = hold.allotment {
            said.push_str(&format!(", its share {}", text(status, allotment)));
        }
        said
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

    // A move names its hub first, by the scheme over it, always: status
    // shows them all.
    #[test]
    fn the_command_line_names_one_move_and_its_hub() {
        let parse = |args: &[&str]| args_of(args).map_err(|error| error.to_string());
        let hub = "hub-xrpl-testnet-dsusd";
        let on = |action| Ok(Command::Move(action, hub.into()));
        assert_eq!(parse(&[]), Ok(Command::Status));
        assert_eq!(parse(&["status"]), Ok(Command::Status));
        assert_eq!(
            parse(&["deposit", hub, "3"]),
            on(Move::Deposit(3_000_000_000))
        );
        assert_eq!(
            parse(&["withdraw", hub, "0.5"]),
            on(Move::Withdraw(500_000_000))
        );
        assert_eq!(parse(&["auto", hub, "off"]), on(Move::Automatic(false)));
        assert_eq!(
            parse(&["target", hub, "10"]),
            on(Move::Target(Some(10_000_000_000)))
        );
        assert_eq!(parse(&["target", hub, "default"]), on(Move::Target(None)));
        assert_eq!(parse(&["give-up", hub]), on(Move::GiveUp));
        assert_eq!(parse(&["leave", hub]), on(Move::Leave));
        for wrong in [
            &["deposit", "3"][..],
            &["deposit", hub],
            &["deposit", "3", hub],
            &["withdraw", "0.5"],
            &["auto", "off"],
            &["auto", hub, "maybe"],
            &["target", hub],
            &["target", "10"],
            &["target", hub, "0"],
            &["target", hub, "none"],
            &["target", hub, "10", "default"],
            &["give-up"],
            &["give-up", "xrpl-testnet-xrp"],
            &["give-up", hub, hub],
            &["give-up", "--hub", hub],
            &["leave"],
            &["leave", "xrpl-testnet-dsusd"],
            &["leave", hub, hub],
            &["status", "now"],
            &["status", hub],
        ] {
            assert!(parse(wrong).is_err(), "{wrong:?}");
        }
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
            target: 5_200_000_000,
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
            "near 5.2 XRP, the default (`dassie hub target  <amount>` sets yours): between 0.4 \
             XRP and 10 XRP, what is beyond going to your own account; the node moves funds by \
             itself"
        );
        status.target_by_owner = true;
        assert!(keeps(&status, 0).starts_with("near 5.2 XRP, your target: between 0.4 XRP"));
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

    // spec/decisions/hold-allotment.md: a hold of ours toward a peer says
    // the peer's share; a peer's toward us, and one toward an account the
    // node does not peer with, have none.
    #[test]
    fn holds_say_their_allotment() {
        let status = proto::HubStatus {
            scheme: "hub-xrpl-testnet-xrp".into(),
            ..Default::default()
        };
        let paying = [
            proto::HubHold {
                peer: "dA".into(),
                account: 7,
                amount: 500_000_000,
                allotment: Some(450_000_000),
            },
            proto::HubHold {
                account: 9,
                amount: 300_000_000,
                ..Default::default()
            },
        ];
        let said = holds(&status, &paying, "to");
        let lines: Vec<&str> = said.lines().map(str::trim).collect();
        assert_eq!(
            lines,
            [
                "0.5 XRP to dA (account 7), its share 0.45 XRP",
                "0.3 XRP to account 9"
            ]
        );
        assert_eq!(holds(&status, &[], "from"), "no holds");
    }

    // spec/decisions/slots-by-liquidity.md: a pool that covers no more
    // peerings says which newcomers the node refuses, and a short one when
    // it ends some.
    #[test]
    fn a_short_pool_says_whom_it_refuses() {
        let mut liquidity = proto::HubLiquidity {
            pool: 300_000_000,
            peerings: 3,
            covered: 4,
            ..Default::default()
        };
        let mut status = proto::HubStatus {
            account: Some(5),
            connection: proto::LedgerConnection::Synced.into(),
            validators: 4,
            validators_answering: 4,
            liquidity: Some(liquidity),
            ..Default::default()
        };
        assert!(brief(&status).ends_with("4 of 4 validators answering"));
        assert_eq!(
            liquidity_text(&status, &liquidity),
            "0.3 XRP covers its 3 peerings at a largest packet each, and more; takes new peers"
        );
        liquidity.covered = 3;
        liquidity.refusing_downlinks = true;
        status.liquidity = Some(liquidity);
        assert!(brief(&status).ends_with(
            "; takes no new downlinks: its pool covers 3 of its 3 peerings at a largest packet \
             each, and no more"
        ));
        liquidity.covered = 1;
        liquidity.refusing_core_peers = true;
        liquidity.short_since_unix_ms = Some(0);
        liquidity.ends_from_unix_ms = Some(60_000);
        let full = liquidity_text(&status, &liquidity);
        assert!(full.starts_with("0.3 XRP: takes no new peers: its pool covers 1 of its 3"));
        assert!(
            full.contains("still short at 1970-01-01 00:01 UTC"),
            "{full}"
        );
        assert!(!full.contains("  "), "{full}");
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
