//! A simulated network of real engine cores
//! (spec/areas/testing.md#deterministic-simulation), for this crate's tests and
//! the simulation runner, dassie-sim
//! (spec/decisions/random-seed-simulations.md): each node's core with its own
//! in-memory database, wired through a virtual network with latency, loss and
//! a forger sending copies of requests as its own, each on a clock of its
//! own, crashed and restarted from
//! its database, paying over STREAM through BTP clients. Everything comes
//! from one seed.
//!
//! It checks what only the whole node can show: peerings and balances
//! survive restarts, payments of several times the credit cross hops, so
//! that every hop settles in a run that settles, an owner's termination
//! ends a peering with nothing settled for its end
//! (spec/decisions/ending-a-peering.md), and
//! the stubs' "underlying ledger" neither makes nor loses money. One
//! payment's token has a spending cap, which the journal must show it
//! kept to.
//!
//! Each node is judged on its own, by its own books
//! (spec/decisions/own-books.md): they hold what its scheme holds and what
//! its clients and receivers really moved, it takes every settlement its
//! peers send, and it settles whatever its books say it owes past its
//! threshold. No check compares two nodes' books, or adds them up: each keeps
//! its own, and they may drift. How far they drifted, the simulation tells
//! from the Fulfills it saw cross the network and never taken, and what
//! lost each (spec/decisions/crash-drift.md).
//!
//! A node also crashes mid-payment: a sender, a hop or a receiver. Each
//! node books the Fulfills it sent as final, whether or not its upstream
//! took them (spec/decisions/own-fulfills.md), and its books still hold
//! against what its own clients and receivers moved.
//!
//! Outgoing payments go between nodes and to a node's own receiver: each
//! sends exactly its debit amount, across a crash of its node too, and
//! its receiver gets what it sent.
//!
//! `Settling::Trusting` runs the stub with unlimited trust: its peers never
//! run out of room and never settle, for runs whose subject is not
//! settlement: the targeted tests', the scripted `node-stub` runs' and a
//! share of the random runs'. Runs about settlement settle over the stub
//! with commitments, `Settling::StubHub`, or a hub run in the simulation,
//! `Settling::Hub` (spec/areas/testing.md#levels).
//!
//! The plain stub, `Settling::Stub`, settles directly, as do the XRP
//! Ledger's schemes here: only the tests of direct settlement, and the
//! scripted runs on those schemes, still run on them, and go with it
//! (spec/roadmap.md#hubs-only). Such a run settles over the plain stub, or
//! over the XRP Ledger testnet scheme against a fake ledger in the same
//! process: its connections lag, lose requests and drop, and the books
//! must still match every node's account, less its reserve, with no XRP
//! made or lost. A run in the live
//! realm settles over mainnet's schemes, `xrpl-xrp` and `xrpl-rlusd`, against
//! the fake ledger's mainnet, which has no faucet: each node's owner
//! deposits (spec/decisions/mainnet-schemes.md).
//!
//! Runs across currencies give some nodes dollars, some XRP and
//! some both, at a fixed snapshot of rates, one charging a fee and a
//! spread: BTP clients pay receivers in the other currency, through the
//! nodes that convert, which price what they convert by their inventory
//! (spec/decisions/passive-rebalancing.md). No ledger makes or loses any
//! asset, and after every step no node's price is above spot by more than
//! its curve allows, takes anything out of a ledger it holds nothing on, or
//! is better than at what its owner has posted alone.
//!
//! Runs on RLUSD settle in RLUSD of the fake ledger's mainnet, in the live
//! realm (spec/areas/xrpl.md#rlusd), alone or across XRP and RLUSD, with
//! both schemes on one account; runs across XRP and DSUSD do the same in
//! the fake ledger's testnet DSUSD (spec/areas/xrpl.md#dsusd). The issuer
//! mints each node its token once its line exists, then claws some back
//! and freezes lines, deep or for every holder; each node's books count
//! what the issuer did as its owner's deposits and withdrawals, and hold
//! its line. The token's pool stays at a dollar at the run's rates, which
//! nodes learn every minute, so its peg guard lets it settle throughout
//! (spec/decisions/issued-assets.md).
//!
//! A node that never settles, or spends the credit it was given and
//! vanishes, costs its peer no more than that credit
//! (spec/decisions/own-fulfills.md).
//!
//! A node's commit can be made to fail, the one fault injected at the
//! store (spec/decisions/random-seed-simulations.md): its core stops without
//! handing over anything of the step, as it does in the daemon, and the node
//! is down until restarted from its database, as after a crash.
//!
//! Test code, behind the `testing` feature: a panic here is a failed run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

pub mod checks;
pub mod hub;
pub mod hub_pairs;
pub mod hub_runs;
pub mod hub_token;
pub mod hubbed;
pub mod scripted;
pub mod swarm;

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Once};

use dassie_connector::SpendingCap;
use dassie_identity::{NodeId, NodeKey};
use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_kit::sim::SeededRng;
use dassie_ledger::LedgerId;
use dassie_node_core::config::{LiveRealm, Realm};
use dassie_oer::Decode;
use dassie_payments::amount::paid_in_full;
use dassie_peer::wire::SchemeId;
use dassie_peer::{Bootstrap, Failure, InboundId, OwnInfo, RequestId, Response};
use dassie_settlement::policy;
use dassie_stream::{AssetDetails, BUFFER_LENGTH, Config, Connection, Policy, Role};
use dassie_xrpl::DROPS_PER_XRP;
use dassie_xrpl::account::{Doubt, LEDGER_SILENCE_MS, Proofs, REQUEST_TIMEOUT_MS, STALL_MS};
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::TokenValue;
use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger, Lie};
use dassie_xrpl::network::{MAINNET, Network, TESTNET};

use crate::engine::{
    Core, CoreParts, EngineEvent, Event, LedgerEvent, NewIncoming, NewOutgoing, Outbound, Output,
    SettlementEvent, Severity,
};
use crate::node::Policies;
use crate::store::{Aim, FailedCommit, ForeignAmount, Holds, PaymentId, PeerSettings, Store};

/// When every simulation starts.
pub const START: u64 = 1_800_000_000_000;
/// The credit a node extends by default.
pub const CREDIT: i128 = 1_100_000_000;
/// Every node's trigger, one and a half of the largest packets
/// (spec/decisions/thresholds-in-packets.md).
pub const TRIGGER: u64 = policy::trigger(Policies::DEFAULT.connector.max_packet_amount);
/// How long a payment's client waits to measure the exchange rate again,
/// after a measurement failed or a rate too low stopped its money, or to
/// connect again, after its connection closed.
pub const RATE_RETRY_MS: u64 = 5_000;
/// How long a node's connection to the ledger takes to come back after it
/// dropped, as the driver's would.
pub const LEDGER_RECONNECT_MS: u64 = 5_000;
/// The longest a server lies to a node before its connection drops again:
/// twice the longest of the production timings that end a connection, so
/// that a node meets the lie however it asks, and some lies end before the
/// node leaves for one it caught while others outlast it.
pub const LONGEST_LIE_MS: u64 = 2 * {
    let mut longest = STALL_MS;
    if REQUEST_TIMEOUT_MS > longest {
        longest = REQUEST_TIMEOUT_MS;
    }
    if LEDGER_SILENCE_MS > longest {
        longest = LEDGER_SILENCE_MS;
    }
    longest
};
/// How often the fake ledger closes a ledger.
pub const LEDGER_CLOSE_MS: u64 = 3_500;
/// The shortest and the longest a message takes in a run over a hub: its
/// validators and members are in one region, within Δ of each other
/// (`dassie_hub::consensus::Config`).
pub const REGIONAL_LATENCY_MS: (u64, u64) =
    (1, dassie_hub::consensus::Config::DEFAULT.max_delay_ms);
/// The longest a forged copy of a request arrives after the original.
pub const FORGERY_WITHIN_MS: u64 = 90_000;

/// The key of the forger, who saw requests between nodes and sends them
/// again under the only key it can prove, its own: no node of the network
/// (spec/areas/peer-protocol.md#authentication).
pub fn forger() -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(&[0xf0; 32]))
}

/// Whether a request of `kind` may be taken from anyone, the forger
/// included: one that only reads, a link state, signed by its node and
/// relayed by anyone, or a commitment to a name, whose holder is sealed in
/// it. Any other is taken only from a node the recipient holds something
/// with, as that node's own.
const fn anyone_may_send(kind: dassie_peer::wire::Kind) -> bool {
    use dassie_peer::wire::Kind;
    kind.is_read_only() || matches!(kind, Kind::LinkStateUpdate | Kind::NameCommit)
}

/// What a run settles over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settling {
    /// The plain stub scheme, in dollars, which settles directly: for the
    /// tests of direct settlement, which go with it
    /// (spec/roadmap.md#hubs-only).
    Stub,
    /// The stub scheme with unlimited trust, in dollars: peers extend each
    /// other the unlimited credit and never settle, for runs whose subject
    /// is not settlement (spec/areas/settlement.md#the-stub-scheme).
    Trusting,
    /// The stub scheme with commitments, at most this many packets in
    /// flight per peer each way: the packet path over a settlement hub,
    /// without a hub (spec/areas/settlement.md#the-stub-scheme).
    StubHub(usize),
    /// The XRP Ledger testnet scheme, against the fake ledger.
    Xrpl,
    /// The XRP Ledger mainnet scheme, `xrpl-xrp`, in the live realm, against
    /// the fake ledger's mainnet.
    XrplLive,
    /// Node 0 on the stub, 2 on the XRP Ledger, 1 and 3 on both.
    Cross,
    /// The RLUSD scheme alone, `xrpl-rlusd`, in the live realm, against the
    /// fake ledger's mainnet and its issuer: the account's XRP is gas, off
    /// the books.
    Rlusd,
    /// Node 0 on RLUSD, 2 on XRP, 1 and 3 on both, through one account, in
    /// the live realm on the fake ledger's mainnet: the RLUSD scheme's gas
    /// is on the XRP ledger's books there.
    XrpRlusd,
    /// As [`Self::XrpRlusd`], in DSUSD and XRP on the testnet.
    XrpDsusd,
    /// A settlement hub of DSUSD of this many validators, run in the
    /// simulation, whose escrow holds DSUSD on the fake ledger: each node
    /// runs the owner books of XRP and DSUSD on its XRP Ledger testnet
    /// account, peering on neither, funds its account in the hub from the
    /// DSUSD book, which the fake ledger's issuer mints, and peers over the
    /// hub (spec/decisions/token-hubs.md).
    HubDsusd(usize),
    /// A settlement hub of this many validators, run in the simulation,
    /// whose escrow is on the fake ledger: each node funds its account in
    /// the hub from its XRP Ledger testnet account, and peers over the hub
    /// (spec/areas/hubs.md#in-the-node).
    Hub(usize),
    /// Across currencies over hubs of this many validators each: node 0 in
    /// the DSUSD hub, as [`Self::HubDsusd`], node 2 in the XRP hub, as
    /// [`Self::Hub`], nodes 1 and 3 in both
    /// (spec/areas/testing.md#over-a-settlement-hub).
    HubAcross(usize),
    /// Two settlement hubs of this many validators each, the XRP hub and
    /// the DSUSD hub, every node a member of both, peering over both: each
    /// runs the owner books of XRP and DSUSD, peering on neither, and funds
    /// each hub from the book of its asset
    /// (spec/areas/testing.md#over-a-settlement-hub).
    Hubs(usize),
}

/// The stub ledger, in dollars.
pub const USD: LedgerId = LedgerId::StubUsd;
/// The XRP Ledger testnet.
pub const XRP: LedgerId = LedgerId::XrplTestnetXrp;
/// The XRP Ledger mainnet: real money, in the live realm.
pub const LIVE_XRP: LedgerId = LedgerId::XrplXrp;
/// RLUSD on the XRP Ledger mainnet, in dollars: real money, in the live
/// realm.
pub const RLUSD: LedgerId = LedgerId::XrplRlusdUsd;
/// DSUSD on the XRP Ledger testnet, in dollars.
pub const DSUSD: LedgerId = LedgerId::XrplTestnetDsusdUsd;
/// XRP in a settlement hub on the XRP Ledger testnet.
pub const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
/// DSUSD in a settlement hub on the XRP Ledger testnet, in dollars.
pub const HUB_DSUSD: LedgerId = LedgerId::HubXrplTestnetDsusdUsd;

/// The config of the testnet's XRPL account, which its schemes settle
/// through (spec/areas/settlement.md#external-assets).
const XRPL_CONFIG: &str =
    r#"{"server":"ws://ledger.test","faucet":"https://faucet.test/accounts"}"#;
/// The config of mainnet's account: mainnet has no faucet.
const XRPL_LIVE_CONFIG: &str = r#"{"server":"ws://ledger.test"}"#;
/// The secret the fake ledger's validators sign under, and the nodes check
/// their validations by: a keyed hash, much cheaper than signatures, and as
/// unforgeable to a server without it
/// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
const VALIDATION_SECRET: [u8; 32] = *b"dassie simulation validations ok";

/// What the fake ledger's issuer mints each node on its token's ledger once
/// its line exists, in millionths of a dollar: as much as the stub gives.
pub const MINT_MICROS: u64 = 100_000_000;
/// What a token's pool holds of it, in whole tokens: beside twice as many
/// XRP, a dollar at [`RATES`], and deep enough for either network's floor.
pub const POOL_TOKENS: u64 = 100_000;

impl Settling {
    /// The ledger a run of one ledger settles over.
    pub fn ledger(self) -> LedgerId {
        match self {
            Self::Stub | Self::Trusting | Self::StubHub(_) => USD,
            Self::Xrpl => XRP,
            Self::XrplLive => LIVE_XRP,
            Self::Rlusd => RLUSD,
            Self::HubDsusd(_) => HUB_DSUSD,
            Self::Hub(_) => HUB,
            Self::Cross | Self::XrpRlusd | Self::XrpDsusd => {
                panic!("a run across currencies has two ledgers")
            }
            Self::HubAcross(_) | Self::Hubs(_) => panic!("a run over two hubs has two ledgers"),
        }
    }

    /// The ledgers of node `index`: the one it pays from first.
    pub fn ledgers(self, index: usize) -> &'static [LedgerId] {
        match (self, index % 4) {
            (Self::Stub | Self::Trusting | Self::StubHub(_), _) | (Self::Cross, 0) => &[USD],
            (Self::Xrpl, _) | (Self::Cross | Self::XrpDsusd, 2) => &[XRP],
            (Self::XrplLive, _) | (Self::XrpRlusd, 2) => &[LIVE_XRP],
            (Self::Cross, _) => &[USD, XRP],
            (Self::Rlusd, _) | (Self::XrpRlusd, 0) => &[RLUSD],
            (Self::XrpRlusd, _) => &[RLUSD, LIVE_XRP],
            (Self::XrpDsusd, 0) => &[DSUSD],
            (Self::XrpDsusd, _) => &[DSUSD, XRP],
            (Self::HubDsusd(_), _) => &[HUB_DSUSD],
            (Self::Hub(_), _) => &[HUB],
            (Self::HubAcross(_), 0) => &[HUB_DSUSD],
            (Self::HubAcross(_), 2) => &[HUB],
            (Self::HubAcross(_) | Self::Hubs(_), _) => &[HUB_DSUSD, HUB],
        }
    }

    /// Whether its nodes settle on the fake XRP Ledger.
    pub const fn on_xrpl(self) -> bool {
        !self.on_stub()
    }

    /// Whether its nodes settle on the stub alone.
    pub const fn on_stub(self) -> bool {
        matches!(self, Self::Stub | Self::Trusting | Self::StubHub(_))
    }

    /// The ledger of the token some of its nodes settle in, and the token,
    /// which the fake ledger's issuer mints.
    pub const fn token(self) -> Option<(LedgerId, IssuedAsset)> {
        match self {
            Self::Rlusd | Self::XrpRlusd => Some((RLUSD, IssuedAsset::RLUSD)),
            Self::XrpDsusd | Self::HubDsusd(_) | Self::HubAcross(_) | Self::Hubs(_) => {
                Some((DSUSD, IssuedAsset::DSUSD))
            }
            Self::Stub
            | Self::Trusting
            | Self::StubHub(_)
            | Self::Xrpl
            | Self::XrplLive
            | Self::Cross
            | Self::Hub(_) => None,
        }
    }

    /// The XRP Ledger network its nodes settle on, if they do.
    pub const fn network(self) -> &'static Network {
        match self {
            Self::XrplLive | Self::Rlusd | Self::XrpRlusd => &MAINNET,
            _ => &TESTNET,
        }
    }

    /// The realm of its nodes: live on mainnet, which they run by the dev
    /// switch, as tests do (spec/decisions/mainnet-schemes.md).
    pub const fn realm(self) -> (Realm, LiveRealm) {
        match self.network().realm {
            dassie_ledger::Realm::Live => (Realm::Live, LiveRealm::DevSwitch),
            dassie_ledger::Realm::Test => (Realm::Test, LiveRealm::Refused),
        }
    }

    /// Whether it runs across currencies, checked by [`checks::check_across`].
    pub const fn across(self) -> bool {
        matches!(
            self,
            Self::Cross | Self::XrpRlusd | Self::XrpDsusd | Self::HubAcross(_) | Self::Hubs(_)
        )
    }

    /// What each node's owner starts with on `ledger`: the stub's balance,
    /// what the faucet gives above the reserve, or what the issuer mints.
    pub const fn deposit(ledger: LedgerId) -> i128 {
        match ledger {
            LedgerId::XrplTestnetXrp | LedgerId::XrplXrp => {
                (FAUCET_DROPS as i128 - 1_000_000) * 1_000
            }
            LedgerId::XrplRlusdUsd | LedgerId::XrplTestnetDsusdUsd => MINT_MICROS as i128 * 1_000,
            _ => 100_000_000_000,
        }
    }

    /// The scheme of `ledger`, and its configuration.
    pub const fn scheme(ledger: LedgerId) -> (&'static str, &'static str) {
        match ledger {
            LedgerId::XrplTestnetXrp => ("xrpl-testnet-xrp", ""),
            LedgerId::XrplXrp => ("xrpl-xrp", ""),
            LedgerId::XrplRlusdUsd => ("xrpl-rlusd", ""),
            LedgerId::XrplTestnetDsusdUsd => ("xrpl-testnet-dsusd", ""),
            LedgerId::HubXrplTestnetDsusdUsd => (dassie_hub::member::DSUSD_SCHEME_ID, ""),
            LedgerId::HubXrplTestnetXrp => (dassie_hub::member::SCHEME_ID, ""),
            _ => ("stub", ""),
        }
    }

    /// The configuration its nodes give the scheme of `ledger`: the stub
    /// with unlimited trust or with commitments in a run over it, or a
    /// hub's rules. In a run over a hub, a member's XRP Ledger testnet
    /// scheme is its owner's book, which funds its account in the hub and
    /// peers with no one (spec/areas/xrpl.md#owner-books); a DSUSD hub's
    /// members peer on neither owner book.
    pub fn config(self, ledger: LedgerId) -> String {
        if let Self::HubDsusd(validators)
        | Self::Hub(validators)
        | Self::HubAcross(validators)
        | Self::Hubs(validators) = self
            && let Some(spec) = hub::spec_of(self, ledger)
        {
            return hub::config_in(&spec, validators);
        }
        match self {
            Self::Trusting if ledger == USD => r#"{"trust":"unlimited"}"#.into(),
            Self::StubHub(limit) if ledger == USD => format!(r#"{{"commitments":{limit}}}"#),
            Self::HubDsusd(_) | Self::Hub(_) | Self::HubAcross(_) | Self::Hubs(_)
                if ledger == XRP || ledger == DSUSD =>
            {
                r#"{"peers":false}"#.into()
            }
            _ => Self::scheme(ledger).1.into(),
        }
    }

    /// The owner books node `index` runs in a run over hubs, peering on
    /// none: the XRP book for any hub, whose XRP pays the XRP Ledger's
    /// fees, and the DSUSD book for a DSUSD hub, which funds it.
    pub fn owner_books(self, index: usize) -> &'static [LedgerId] {
        let ledgers = self.ledgers(index);
        match (ledgers.contains(&HUB), ledgers.contains(&HUB_DSUSD)) {
            (_, true) => &[XRP, DSUSD],
            (true, false) => &[XRP],
            (false, false) => &[],
        }
    }

    /// The XRPL account its nodes' XRPL schemes settle through, if any, and
    /// its config: the fake ledger's server and faucet, trusting its
    /// validators.
    pub fn account_config(self) -> Option<(String, String)> {
        let network = self.network();
        let config = if network.faucet.is_some() {
            XRPL_CONFIG
        } else {
            XRPL_LIVE_CONFIG
        };
        let uses = self.on_xrpl() || matches!(self, Self::HubDsusd(_) | Self::Hub(_));
        uses.then(|| {
            let config = dassie_xrpl::fake::with_publishers(config, network);
            (network.first().to_string(), config)
        })
    }
}

/// The fake ledger of `network`, its clock at the simulation's start and
/// its validations signed under [`VALIDATION_SECRET`].
fn fake_ledger(network: &Network) -> FakeLedger {
    let mut ledger = match network.faucet {
        None => FakeLedger::mainnet(),
        Some(_) => FakeLedger::new(),
    };
    ledger.set_close_time(dassie_xrpl::unl::ledger_seconds(START));
    ledger.sign_validations_with(VALIDATION_SECRET);
    ledger
}

/// A withdrawal of a peer short of room.
#[derive(Clone, Copy, Debug)]
pub struct Withdrawal {
    /// When.
    pub at: u64,
    /// The node that withdrew it.
    pub by: usize,
    /// The peer.
    pub peer: NodeId,
    /// The scheme the pair settles over, if the node still had the
    /// peering.
    pub scheme: Option<SchemeId>,
}

/// What the issuer of the run's token stops, in a run on a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssuerFreeze {
    /// Node `0`'s line: it pays only the issuer.
    Line(usize),
    /// Node `0`'s line, deep: it neither pays nor receives.
    Deep(usize),
    /// Every line: no holder pays another.
    Global,
}

impl IssuerFreeze {
    /// Whether it stops what node `index` pays or is paid.
    pub const fn stops(self, index: usize) -> bool {
        match self {
            Self::Line(node) | Self::Deep(node) => node == index,
            Self::Global => true,
        }
    }
}

/// The rates of runs across currencies and on a token: a dollar buys two
/// XRP. A node learns them when it starts, and again every minute, as a
/// node fetches them, so a token's peg guard reads its pool at a fresh
/// rate.
pub const RATES: [(&str, u128); 2] = [
    ("USD", 1_000_000_000_000_000),
    ("XRP", 2_000_000_000_000_000),
];

/// What node 1 charges for what passes between its peers, in runs across
/// currencies: a 1% fee and a 0.5% spread, with the curve every node starts
/// with.
pub const CHARGES: PeerSettings = PeerSettings {
    connector_fee_ppm: 10_000,
    fx_spread_ppm: 5_000,
    rebalance_ppm: dassie_connector::pricing::DEFAULT_REBALANCE_PPM,
};

/// How far a run across currencies moves a converting node's funds at
/// most, in tenths of what it started with on each ledger: the payments
/// carry a few dollars, and the issuer claws back a few, of a hundred.
const MOST_MOVED_TENTHS: i128 = 1;

/// The least a payment across currencies delivers into `into` for `sent`
/// from `ledger`: what node 1, which charges, gives with its funds moved
/// against the payment as far as a run moves them, and the whole payment
/// priced as one packet there, paying its own impact
/// (spec/decisions/passive-rebalancing.md), less its fee, and 5% for
/// packets' rounding and a second hop's fee. The same currency loses only
/// the 5%.
pub fn least_across(ledger: LedgerId, into: LedgerId, sent: u64) -> u64 {
    use dassie_connector::{Holding, Inventory};
    if ledger.asset().code == into.asset().code {
        return sent / 100 * 95;
    }
    let mut rates = dassie_connector::Rates::new();
    for (code, rate) in RATES {
        rates.set_at(code, rate, START);
    }
    let held = |ledger, tenths| Inventory {
        ledger,
        owner: dassie_ledger::AccountId(0),
        holding: Holding {
            posted: u128::try_from(Settling::deposit(ledger) * tenths / 10).unwrap(),
            ..Holding::NONE
        },
    };
    let inventories = [
        held(ledger, 10 + MOST_MOVED_TENTHS),
        held(into, 10 - MOST_MOVED_TENTHS),
    ];
    let pricing = CHARGES.pricing();
    let max_age_ms = Policies::DEFAULT.connector.max_rate_age_ms;
    let table =
        dassie_connector::PriceTable::build(&rates, &inventories, pricing, START, max_age_ms);
    let net = sent - pricing.charges.fee(sent);
    let holdings = table.holdings();
    let delivered = table.convert(ledger, into, sent, net, &holdings, START, max_age_ms);
    delivered.unwrap() / 100 * 95
}

/// A node's connection to the fake ledger.
#[derive(Clone, Copy, Debug)]
struct LedgerLink {
    epoch: u64,
    connection: u64,
}

/// Node `index`'s key, the same in every run.
pub fn key(index: usize) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(
        &[u8::try_from(index + 40).unwrap(); 32],
    ))
}

/// The ILP packet a peer message carries, with its request ID, if it
/// carries one.
fn ilp_packet(body: &[u8]) -> Option<(u32, dassie_ilp::Packet<'_>)> {
    use dassie_peer::wire::Content;
    if !dassie_peer::wire::peek_kind(body).is_some_and(dassie_peer::wire::Kind::carries_packet) {
        return None;
    }
    // A Prepare comes on a method, over a settlement hub with its
    // commitment, which the simulation does not read.
    let (Content::InterledgerPacket { request_id, packet }
    | Content::MethodPacket {
        request_id, packet, ..
    }) = Content::decode_exact(body).ok()?
    else {
        return None;
    };
    Some((request_id, dassie_ilp::Packet::decode_exact(packet).ok()?))
}

/// Whether a peer message is a Prepare over a settlement hub: on a method
/// that carries the payer's commitment.
fn hub_prepare(body: &[u8]) -> bool {
    matches!(
        dassie_peer::wire::Content::decode_exact(body),
        Ok(dassie_peer::wire::Content::MethodPacket { commitment, .. }) if !commitment.is_empty()
    )
}

/// Node `index`'s URL.
pub fn url(index: usize) -> String {
    format!("https://n{index}.test")
}

/// Where node `index` takes QUIC: its URL's host and port.
#[must_use]
pub fn endpoint(index: usize) -> dassie_peer::wire::Endpoint {
    dassie_peer::wire::Endpoint::of_url(&url(index)).expect("a node URL")
}

enum Delivery {
    Request {
        from: usize,
        generation: u64,
        to: usize,
        id: RequestId,
        body: Vec<u8>,
    },
    Result {
        to: usize,
        generation: u64,
        id: RequestId,
        result: Result<Response, Failure>,
    },
    /// A copy of a request, as the forger who saw it sends it again, under
    /// its own key: its answer goes to no one.
    Forged {
        from: usize,
        to: usize,
        /// When its original arrived.
        arrived: u64,
        body: Vec<u8>,
    },
    /// A request on a node's connection to the ledger.
    LedgerRequest {
        from: usize,
        connection: u64,
        message: Vec<u8>,
    },
    /// What the ledger said, or that the connection is up or down, or what
    /// the faucet did, for a node's connection `epoch`.
    LedgerEvent {
        to: usize,
        generation: u64,
        epoch: u64,
        event: LedgerEvent,
    },
    /// A member's hub account asks a hub's validator.
    HubRequest {
        member: usize,
        generation: u64,
        asset: String,
        epoch: u64,
        endpoint: usize,
        message: Vec<u8>,
    },
    /// A message of a validator's answer to a member.
    HubAnswer {
        member: usize,
        generation: u64,
        asset: String,
        epoch: u64,
        endpoint: usize,
        message: Vec<u8>,
    },
    /// A member's exchange with a validator is over.
    HubExchangeEnd { member: usize, endpoint: usize },
    /// A member's submission to a validator outside its account's
    /// exchanges, as a dishonest member makes them.
    HubSubmit {
        member: usize,
        endpoint: usize,
        message: Vec<u8>,
    },
    /// A message between two validators; a reply is answered by no one.
    ValidatorMessage {
        from: usize,
        to: usize,
        message: Vec<u8>,
        reply: bool,
    },
    /// A request of a validator's escrow watcher to the fake ledger.
    ValidatorLedgerRequest {
        validator: usize,
        connection: u64,
        message: Vec<u8>,
    },
    /// What the fake ledger said to a validator's watcher, or, without a
    /// message, that its connection is up.
    ValidatorLedgerEvent {
        validator: usize,
        generation: u64,
        connection: u64,
        message: Option<Vec<u8>>,
    },
    /// The state a validator that is behind fetched from another.
    ValidatorState {
        validator: usize,
        generation: u64,
        finalized: Box<dassie_hub::consensus::Finalized>,
        state: Vec<u8>,
    },
    /// A node's connection to a lying server ends, if it is still open:
    /// the next is honest.
    Honest { to: usize, connection: u64 },
}

/// A payment by a BTP client of one node to the receiver of another.
pub struct Payment {
    /// The node whose client pays.
    pub from: usize,
    /// The node whose receiver is paid.
    pub to: usize,
    /// The incoming payment it pays.
    pub incoming: PaymentId,
    /// The most that incoming payment takes, if it has a limit.
    pub limit: Option<u64>,
    /// The token's ID, and its cap if it has one.
    pub token: (u64, Option<SpendingCap>),
    /// The ledger it pays from.
    pub ledger: LedgerId,
    link: u64,
    /// The client's STREAM connection.
    pub connection: Connection,
    /// Its address, the receiver's, and the shared secret: what a new
    /// connection needs when the old one closed.
    credentials: (String, String, [u8; 32]),
    /// What earlier connections sent.
    sent_before: u64,
    /// How much it sends.
    pub amount: u64,
    stream: Option<u64>,
    /// BTP request ID → STREAM request ID.
    in_flight: BTreeMap<u32, u64>,
    next_request: u32,
    /// Whether it sent all it meant to.
    pub done: bool,
    /// Whether its incoming payment expired before it got all it asked
    /// for, so that its client stopped there.
    pub expired: bool,
    /// When its client measures the exchange rate again, after a
    /// measurement failed or a rate too low stopped its money, or connects
    /// again, after its connection closed.
    rate_retry_at: Option<u64>,
}

/// An ILP Prepare by its sender, its peer, its request ID and its
/// condition.
type PacketKey = (usize, usize, u32, [u8; 32]);

/// An ILP Prepare a node sent a peer, until it is known whether a Fulfill
/// the peer booked for it reached the sender's books
/// (spec/decisions/crash-drift.md).
struct Sent {
    amount: u64,
    /// Its expiry, on the sender's clock.
    expires_at_ms: u64,
    /// The sender's generation when it sent it: a sender that restarted
    /// since lost it.
    generation: u64,
    /// Whether the peer sent a Fulfill for it, which it booked first.
    fulfilled: bool,
    /// What lost it for good, if anything did yet.
    lost: Option<Lost>,
}

/// What lost a Prepare's Fulfill for good, before it was past being taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lost {
    /// A crash or a failed commit of the sender, while it could still take
    /// the Fulfill: its connector forgot the packet.
    SenderCrash,
    /// A crash or a failed commit of the peer, while its Fulfill could
    /// still arrive: it sends it no more. A copy already on its way may
    /// still arrive, and be refused or late.
    PeerCrash,
    /// The sender refused the peer's Fulfill with 400, 401 or 409.
    Refused,
    /// The Fulfill reached the sender after its expiry, or after it
    /// restarted.
    Late,
}

impl Lost {
    /// Records in `lost` that `cause` lost a Fulfill, unless something
    /// already did. The sender's crash, a refusal or a late arrival takes
    /// the place of the peer's crash, since the peer's copy on its way
    /// could still have arrived; otherwise the first cause stays.
    fn record(lost: &mut Option<Self>, cause: Self) {
        if lost.is_none() || (*lost == Some(Self::PeerCrash) && cause != Self::PeerCrash) {
            *lost = Some(cause);
        }
    }
}

/// How far two peers' books drifted apart, by what really happened: the
/// Fulfills one booked that never reached the other's books
/// (spec/decisions/own-fulfills.md), with what crashes lost apart from the
/// rest (spec/decisions/crash-drift.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// What crashes and failed commits lost.
    pub crashes: u128,
    /// What else was lost: Fulfills that were refused, arrived late, or
    /// never got across a cut.
    pub others: u128,
}

/// The most Prepares the simulation follows at once, more than the
/// network ever has in flight.
const MAX_SENT: usize = 1 << 20;

/// A node: its database, and its core while it runs.
pub struct SimNode {
    /// Its database, which outlives a crash.
    pub store: Arc<Store>,
    /// Its engine core, or `None` while it is down.
    pub core: Option<Core>,
    /// How far its clock is ahead of the network's, or behind it.
    pub skew_ms: i64,
    generation: u64,
    next_inbound: u64,
    /// Inbound peer requests waiting for the core's reply.
    inbound: BTreeMap<InboundId, (usize, u64, RequestId)>,
    /// The forger's copies waiting for the core's reply, which goes
    /// nowhere: their kinds.
    forged: BTreeMap<InboundId, Option<dassie_peer::wire::Kind>>,
    /// The commit it is to fail, while it has one.
    failing: Option<FailedCommit>,
    /// Inbound Fulfills of Prepares it sent, waiting for the core's reply.
    fulfills: BTreeMap<InboundId, PacketKey>,
}

/// The simulated network: its nodes, its clock, its messages, the fake
/// ledger, and the payments under way.
pub struct Sim {
    /// The network's clock.
    pub now: u64,
    /// Every node, by index.
    pub nodes: Vec<SimNode>,
    bootstrap: Vec<Bootstrap>,
    queue: BTreeMap<(u64, u64), Delivery>,
    sequence: u64,
    rng: SeededRng,
    /// How many of a thousand messages are lost.
    pub loss_per_mille: u64,
    /// How many of a thousand requests that arrive the forger sends again,
    /// up to [`FORGERY_WITHIN_MS`] later, under its own key ([`forger`]).
    pub forgery_per_mille: u64,
    /// The nodes the issuer is to mint the run's token once their line
    /// exists.
    to_mint: BTreeSet<usize>,
    /// What the issuer minted each node of its token, and what it clawed
    /// back,
    /// in the ledger's internal units: the owner's deposits and
    /// withdrawals there.
    pub issued: BTreeMap<usize, (i128, i128)>,
    /// Each freeze of the issuer, from when it applied to when it was
    /// lifted, if it was.
    pub freezes: Vec<(IssuerFreeze, u64, Option<u64>)>,
    /// Each stretch in which the ledger's servers or validators failed a
    /// node, or every node, from when it began to when it ended: a server
    /// lying to one node, or a quorum withheld or a list lapsed for all.
    pub outages: Vec<(Option<usize>, u64, u64)>,
    /// The payments under way or done, of nodes that did not crash.
    pub payments: Vec<Payment>,
    /// Payments whose sender's node crashed: its client is gone.
    pub abandoned: Vec<Payment>,
    /// Outgoing payments: from which node, to which, the incoming payment
    /// they pay and the outgoing payment.
    pub outgoing: Vec<(usize, usize, PaymentId, PaymentId)>,
    /// Pairs whose requests do not get through, from the first to the
    /// second.
    pub cut: BTreeSet<(usize, usize)>,
    /// While the network is partitioned, the nodes on one side of it: no
    /// request gets from a node on one side to a node on the other.
    pub apart: BTreeSet<usize>,
    /// The credit each node extends to its peers, where not the default.
    credits: BTreeMap<usize, u64>,
    /// What the run settles over.
    pub settling: Settling,
    /// The fake XRP Ledger, and each node's connection to it.
    pub ledger: FakeLedger,
    links: BTreeMap<usize, LedgerLink>,
    /// When the last message on each node's connection arrives, each way.
    fifo: BTreeMap<(usize, bool), u64>,
    next_connection: u64,
    next_close: u64,
    /// When the nodes next learn [`RATES`] again, in a run that needs them.
    next_rates: u64,
    /// What happened, a line per event, to compare runs by.
    pub trace: Vec<String>,
    /// What the nodes reported as errors, or the events they lost.
    pub errors: Vec<String>,
    /// Outgoing payments a node refused although its owner could pay them.
    pub refused: Vec<String>,
    /// Prices a node set beyond what its curve allows
    /// (spec/decisions/passive-rebalancing.md).
    pub priced: Vec<String>,
    /// Packets a node's stub with commitments held past its connector
    /// ([`Sim::watch_hub`]).
    pub hub_leaks: Vec<String>,
    /// Credit over the stub with commitments apart from the holds
    /// ([`Sim::watch_hub`]).
    pub hub_credit_off: Vec<String>,
    /// Nodes whose connection to the ledger carries nothing.
    pub ledger_down: BTreeSet<usize>,
    /// Each node's connection to a lying server, while it has one.
    lying: BTreeMap<usize, u64>,
    /// What each node's XRPL accounts proved, and how many lies they
    /// caught, as last counted, with the generation of the core that did:
    /// a core counts from nothing when it starts.
    proofs_seen: BTreeMap<usize, (u64, Proofs, u64)>,
    /// Differences between a node's books and its ledger that no reserve
    /// explains (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
    pub unexplained: Vec<String>,
    /// The peer protocol's settings.
    peer: dassie_peer::Config,
    /// Each withdrawal of a peer short of room.
    pub withdrawn: Vec<Withdrawal>,
    /// The shortest and the longest a message takes, in milliseconds.
    pub latency_ms: (u64, u64),
    /// How many times the run reached each thing it counts
    /// (spec/decisions/random-seed-simulations.md).
    pub counts: BTreeMap<&'static str, u64>,
    /// The nodes stepped, started or stopped since
    /// [`Self::run_until_or`] last asked for their deadlines.
    touched: BTreeSet<usize>,
    /// How many times the faucet funded each node: again after a crash cut
    /// its first request short.
    pub fundings: BTreeMap<usize, u64>,
    /// Outgoing payments that may end failed: those whose payer gave up,
    /// as it does after its reconnects, or was reconnecting, while the
    /// network was faulty.
    pub given_up: BTreeSet<PaymentId>,
    /// The nodes that are to fail a commit for [`Sim::fail_commit_anywhere`]:
    /// once one of them has, the others let their commits through.
    anywhere: BTreeSet<usize>,
    /// The Prepares on their way, until their sender took the Fulfill, or
    /// was refused, or could no longer take it.
    sent: BTreeMap<PacketKey, Sent>,
    /// When each Prepare in [`Sim::sent`] is past being taken, on the
    /// network's clock.
    finals: BTreeSet<(u64, PacketKey)>,
    /// The requests that carry a Fulfill, by their sender, its generation
    /// and the request's ID, until their result arrives: a refusal loses
    /// it.
    fulfills_sent: BTreeMap<(usize, u64, RequestId), PacketKey>,
    /// How far each pair of nodes' books drifted, the lower index first.
    drift: BTreeMap<(usize, usize), Drift>,
    /// The settlement hub of a run over one, with its validators: the
    /// first of a run over two.
    pub hub: Option<Box<hub::HubNet>>,
    /// The second settlement hub of a run over two ([`hub::specs`]).
    pub second_hub: Option<Box<hub::HubNet>>,
    /// The member request a validator is answering in the step under way:
    /// the member, its generation, its hub account and connection, and the
    /// validator.
    answering: Option<(usize, u64, String, u64, usize)>,
    /// What each node booked as lost on each peer over a hub
    /// (`SettlementEvent::PeerLoss`), by node and peer.
    pub hub_losses: BTreeMap<(usize, String), u64>,
    /// The most each node's hold toward each peer over a hub held, and the
    /// most the peer's hold toward it held, as its scheme read them.
    pub hub_most_held: BTreeMap<(usize, String), (u64, u64)>,
}

// Every run, whatever it checks besides, ends without an error event: a
// node reports one only for what should not happen
// (spec/decisions/tigerstyle.md).
impl Drop for Sim {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            checks::check_events(self).unwrap();
        }
    }
}

impl Sim {
    /// `count` nodes over the stub with unlimited trust, the first
    /// `bootstraps` of them bootstrap nodes: for a run whose subject is not
    /// settlement (spec/areas/testing.md#levels).
    pub fn new(seed: u64, count: usize, bootstraps: usize) -> Self {
        Self::with(seed, count, bootstraps, BTreeMap::new(), Settling::Trusting)
    }

    /// `count` nodes settling over `settling`, extending `credits` where
    /// not the default.
    pub fn with(
        seed: u64,
        count: usize,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
    ) -> Self {
        Self::with_peer(
            seed,
            count,
            bootstraps,
            credits,
            settling,
            Policies::DEFAULT.peer,
        )
    }

    /// As [`Sim::with`], with the peer protocol's settings `peer`.
    pub fn with_peer(
        seed: u64,
        count: usize,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
        peer: dassie_peer::Config,
    ) -> Self {
        Self::with_skews(seed, bootstraps, credits, settling, peer, &vec![0; count])
    }

    /// As [`Sim::with_peer`], a node for each of `skews_ms`, its clock that
    /// far ahead of the network's, or behind it.
    pub fn with_skews(
        seed: u64,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
        peer: dassie_peer::Config,
        skews_ms: &[i64],
    ) -> Self {
        let count = skews_ms.len();
        let bootstrap = (0..bootstraps)
            .map(|index| Bootstrap {
                id: key(index).node_id(),
                endpoint: endpoint(index),
                key: key(index).public_key(),
            })
            .collect();
        let mut sim = Self {
            now: START,
            nodes: Vec::new(),
            bootstrap,
            queue: BTreeMap::new(),
            sequence: 0,
            rng: SeededRng::new(seed),
            loss_per_mille: 10,
            forgery_per_mille: 0,
            to_mint: (0..skews_ms.len())
                .filter(|index| {
                    // A DSUSD hub's members hold DSUSD in their owner book.
                    settling.owner_books(*index).contains(&DSUSD)
                        || settling
                            .token()
                            .is_some_and(|(ledger, _)| settling.ledgers(*index).contains(&ledger))
                })
                .collect(),
            issued: BTreeMap::new(),
            freezes: Vec::new(),
            outages: Vec::new(),
            payments: Vec::new(),
            abandoned: Vec::new(),
            outgoing: Vec::new(),
            cut: BTreeSet::new(),
            apart: BTreeSet::new(),
            credits,
            settling,
            ledger: fake_ledger(settling.network()),
            links: BTreeMap::new(),
            fifo: BTreeMap::new(),
            next_connection: 0,
            next_close: START + LEDGER_CLOSE_MS,
            next_rates: START + Policies::DEFAULT.rates_every_ms,
            trace: Vec::new(),
            errors: Vec::new(),
            refused: Vec::new(),
            priced: Vec::new(),
            hub_leaks: Vec::new(),
            hub_credit_off: Vec::new(),
            ledger_down: BTreeSet::new(),
            lying: BTreeMap::new(),
            proofs_seen: BTreeMap::new(),
            unexplained: Vec::new(),
            peer,
            withdrawn: Vec::new(),
            latency_ms: (1, 200),
            counts: BTreeMap::new(),
            touched: BTreeSet::new(),
            fundings: BTreeMap::new(),
            given_up: BTreeSet::new(),
            anywhere: BTreeSet::new(),
            sent: BTreeMap::new(),
            finals: BTreeSet::new(),
            fulfills_sent: BTreeMap::new(),
            drift: BTreeMap::new(),
            hub: None,
            second_hub: None,
            answering: None,
            hub_losses: BTreeMap::new(),
            hub_most_held: BTreeMap::new(),
        };
        // The token's pool, at a dollar at the run's rates.
        if let Some((_, token)) = settling.token() {
            let tokens = TokenValue::from_units(POOL_TOKENS, 0).unwrap();
            sim.ledger
                .set_pool(token, 2 * POOL_TOKENS * DROPS_PER_XRP, tokens, 300)
                .unwrap();
        }
        for &skew_ms in skews_ms {
            sim.nodes.push(SimNode {
                store: Arc::new(Store::open_in_memory().unwrap()),
                core: None,
                skew_ms,
                generation: 0,
                next_inbound: 0,
                inbound: BTreeMap::new(),
                forged: BTreeMap::new(),
                failing: None,
                fulfills: BTreeMap::new(),
            });
        }
        if let Settling::HubDsusd(validators)
        | Settling::Hub(validators)
        | Settling::HubAcross(validators)
        | Settling::Hubs(validators) = settling
        {
            sim.latency_ms = REGIONAL_LATENCY_MS;
            sim.found_hub(&vec![dassie_hub::sim::Role::Honest; validators]);
        }
        for index in 0..count {
            sim.start(index);
            if let Some((asset, config)) = settling.account_config() {
                sim.step(index, |core| {
                    core.set_asset_config(&asset, &config).unwrap();
                });
            }
            // A member of a hub funds its account there from its XRP Ledger
            // testnet account, its owner's book, which peers with no one; a
            // DSUSD hub's from the DSUSD book on it, whose gas, and the
            // escrow's, its XRP pays.
            for &book in settling.owner_books(index) {
                let (scheme, config) = (Settling::scheme(book).0, settling.config(book));
                sim.step(index, |core| {
                    core.add_scheme(scheme, config).unwrap();
                });
            }
            for &ledger in settling.ledgers(index) {
                let scheme = Settling::scheme(ledger).0;
                let config = settling.config(ledger);
                sim.step(index, |core| {
                    core.add_scheme(scheme, config).unwrap();
                });
            }
            if settling.across() && index == 1 {
                sim.step(index, |core| core.set_peer_settings(CHARGES).unwrap());
            }
            // Mainnet has no faucet: the owner deposits as much as the
            // testnet's gives.
            if settling.network().faucet.is_none() {
                sim.ledger.deposit(sim.xrpl_account(index), FAUCET_DROPS);
                *sim.fundings.entry(index).or_default() += 1;
            }
        }
        sim
    }

    /// Starts node `index` from its database.
    pub fn start(&mut self, index: usize) {
        let seed = self.rng.next_u64();
        let now = self.local_now(index);
        let node = &mut self.nodes[index];
        node.generation += 1;
        node.inbound.clear();
        node.forged.clear();
        node.fulfills.clear();
        let (realm, live) = self.settling.realm();
        let parts = CoreParts {
            store: Arc::clone(&node.store),
            key: key(index),
            realm,
            live,
            policies: Policies {
                peer_credit: self
                    .credits
                    .get(&index)
                    .copied()
                    .unwrap_or(Policies::DEFAULT.peer_credit),
                peer: self.peer,
                ..Policies::DEFAULT
            },
            rng: Box::new(SeededRng::new(seed)),
            peer_rng: Box::new(SeededRng::new(seed ^ 1)),
            own_info: OwnInfo {
                url: Some(url(index)),
                alias: format!("n{index}"),
                feed_port: None,
                hub: None,
            },
            bootstrap: self.bootstrap.clone(),
            uplinks: Vec::new(),
        };
        let mut core = Core::start(parts, now).unwrap();
        core.check_validations_with(VALIDATION_SECRET);
        node.core = Some(core);
        self.touched.insert(index);
        self.trace.push(format!("{} start {index}", self.now));
        let rates = self.needs_rates().then(|| RATES.map(|(c, r)| (c, r, now)));
        self.step(index, |core| {
            if let Some(rates) = rates {
                core.set_rates(&rates);
            }
        });
    }

    /// Gives every node the first `count` nodes as its bootstrap nodes, as
    /// an owner does in its config, which takes a restart: each running
    /// node stops and starts again from its database.
    pub fn join_network(&mut self, count: usize) {
        self.bootstrap = (0..count)
            .map(|index| Bootstrap {
                id: key(index).node_id(),
                endpoint: endpoint(index),
                key: key(index).public_key(),
            })
            .collect();
        for index in 0..self.nodes.len() {
            if self.nodes[index].core.is_some() {
                self.stop(index);
                self.start(index);
            }
        }
        self.trace
            .push(format!("{} the nodes join the network", self.now));
    }

    /// Crashes node `index`: its clients are gone, its database stays.
    pub fn crash(&mut self, index: usize) {
        self.count("crashes");
        self.stop(index);
        self.trace.push(format!("{} crash {index}", self.now));
    }

    /// Makes node `index` fail the commit after the next `skip` that `aim`
    /// aims at (spec/decisions/random-seed-simulations.md). The node stops in
    /// the step that commits it, and is down until restarted.
    pub fn fail_commit(&mut self, index: usize, aim: Aim, skip: u32) {
        let node = &mut self.nodes[index];
        assert!(node.core.is_some(), "a node that runs fails a commit");
        node.failing = Some(node.store.fail_commit(aim, skip));
        self.trace.push(format!(
            "{} {index} is to fail a commit, after {skip}: {aim:?}",
            self.now
        ));
    }

    /// Makes the first running node to commit what `aim` aims at, after
    /// its next `skip` such commits, fail that commit
    /// (spec/decisions/random-seed-simulations.md): for a moment few nodes
    /// reach at any one time, such as a settlement. Nodes that have a commit
    /// to fail already are left as they are.
    pub fn fail_commit_anywhere(&mut self, aim: Aim, skip: u32) {
        for (index, node) in self.nodes.iter_mut().enumerate() {
            if node.core.is_some() && node.failing.is_none() {
                node.failing = Some(node.store.fail_commit(aim, skip));
                self.anywhere.insert(index);
            }
        }
        self.trace.push(format!(
            "{} a node is to fail a commit, after {skip}: {aim:?}",
            self.now
        ));
    }

    /// Whether node `index` has a commit to fail.
    pub fn failing(&self, index: usize) -> bool {
        self.nodes[index].failing.is_some()
    }

    /// Lets every commit through again: no node fails one any more.
    pub fn let_commits_through(&mut self) {
        for index in 0..self.nodes.len() {
            self.let_through(index);
        }
        self.trace.push(format!("{} commits go through", self.now));
    }

    /// Lets node `index`'s commits through again.
    fn let_through(&mut self, index: usize) {
        let node = &mut self.nodes[index];
        if node.failing.take().is_some() {
            node.store.let_commits_through();
        }
        self.anywhere.remove(&index);
    }

    /// Node `index`'s commit failed, and its core stopped without handing
    /// over anything of the step, as it does in the daemon. What it was
    /// answering gets the 503 the daemon's peer endpoint answers when the
    /// engine stops before its reply (`peer_server.rs`).
    fn commit_failed(&mut self, index: usize, held: Holds) {
        self.count("failed commits");
        if self.anywhere.contains(&index) {
            for other in std::mem::take(&mut self.anywhere) {
                self.let_through(other);
            }
        }
        // What the lost commit held: the floors on these counts prove that
        // a packet's commit and a settlement's commit failed.
        if held.packet {
            self.count("failed commits mid-payment");
        }
        if held.settlement {
            self.count("failed commits mid-settlement");
        }
        self.trace
            .push(format!("{} {index} failed a commit and stopped", self.now));
        let inbound = std::mem::take(&mut self.nodes[index].inbound);
        for (_, (to, generation, id)) in inbound {
            let delay = self.latency();
            let result = Ok(Response {
                status: 503,
                body: b"Service Unavailable".to_vec(),
            });
            self.schedule(
                delay,
                Delivery::Result {
                    to,
                    generation,
                    id,
                    result,
                },
            );
        }
        self.stop(index);
    }

    /// Stops node `index`: its clients are gone, its database stays, and
    /// its commits go through again once it restarts.
    fn stop(&mut self, index: usize) {
        self.touched.insert(index);
        self.nodes[index].core = None;
        self.let_through(index);
        // What it sent, it can no longer take a Fulfill for; what it
        // fulfilled and its sender has not taken, it no longer sends again.
        for (&(from, to, ..), sent) in &mut self.sent {
            let open =
                self.now.saturating_add_signed(self.nodes[from].skew_ms) < sent.expires_at_ms;
            if open && from == index {
                Lost::record(&mut sent.lost, Lost::SenderCrash);
            } else if open && to == index && sent.fulfilled {
                Lost::record(&mut sent.lost, Lost::PeerCrash);
            }
        }
        // A copy it was still working on is lost with the crash: whatever
        // it did, it did as the forger's.
        self.nodes[index].forged.clear();
        if let Some(link) = self.links.remove(&index) {
            self.ledger.disconnect(link.connection);
        }
        let (gone, kept) = std::mem::take(&mut self.payments)
            .into_iter()
            .partition(|payment| payment.from == index);
        self.payments = kept;
        self.abandoned.extend::<Vec<Payment>>(gone);
    }

    fn schedule(&mut self, delay: u64, delivery: Delivery) {
        self.sequence += 1;
        let mut at = self.now + delay;
        // A connection to the ledger is one WebSocket: what goes over it,
        // either way, arrives in order.
        let fifo = match &delivery {
            Delivery::LedgerRequest { from, .. } => Some((*from, false)),
            Delivery::LedgerEvent { to, .. } => Some((*to, true)),
            Delivery::ValidatorLedgerRequest { validator, .. } => {
                Some((hub::VALIDATOR_FIFO + *validator, false))
            }
            Delivery::ValidatorLedgerEvent { validator, .. } => {
                Some((hub::VALIDATOR_FIFO + *validator, true))
            }
            _ => None,
        };
        if let Some(fifo) = fifo {
            let last = self.fifo.entry(fifo).or_insert(0);
            at = at.max(*last);
            *last = at;
        }
        self.queue.insert((at, self.sequence), delivery);
    }

    /// Node `index` answered the forger's copy of a request of `kind`
    /// with `status`. The copy is the forger's own request, and the forger
    /// holds nothing with any node: only what anyone may send may be taken
    /// from it ([`anyone_may_send`]); anything else taken fails the run for
    /// safety.
    fn forgery_answered(
        &mut self,
        index: usize,
        kind: Option<dassie_peer::wire::Kind>,
        status: u16,
    ) {
        self.trace
            .push(format!("{} {index} answers the forgery {status}", self.now));
        if status != 200 {
            self.count("forgeries refused");
            return;
        }
        if kind.is_some_and(anyone_may_send) {
            self.count("forgeries anyone may send");
            return;
        }
        self.errors.push(format!(
            "{} node {index} took the forger's {kind:?}",
            self.now
        ));
    }

    /// The time on node `index`'s clock.
    fn local_now(&self, index: usize) -> u64 {
        self.now.saturating_add_signed(self.nodes[index].skew_ms)
    }

    /// When node `index`'s core is next due, on the network's clock.
    fn deadline(&self, index: usize) -> Option<u64> {
        let local = self.local_deadline(index)?;
        Some(local.saturating_add_signed(-self.nodes[index].skew_ms))
    }

    /// When node `index`'s core is next due, on its own clock.
    fn local_deadline(&self, index: usize) -> Option<u64> {
        self.nodes[index].core.as_ref()?.deadline()
    }

    /// Asks the nodes touched since the last time for their deadlines
    /// again, on their own clocks, and any node started since.
    fn refresh(&mut self, deadlines: &mut Vec<Option<u64>>) {
        deadlines.resize(self.nodes.len(), None);
        for index in core::mem::take(&mut self.touched) {
            deadlines[index] = self.local_deadline(index);
        }
    }

    fn latency(&mut self) -> u64 {
        let (shortest, longest) = self.latency_ms;
        shortest + self.rng.next_u64() % (longest - shortest + 1)
    }

    /// Counts one more of `what`.
    pub fn count(&mut self, what: &'static str) {
        *self.counts.entry(what).or_default() += 1;
    }

    /// Runs `f` on a node's core as one step, then carries out its outputs.
    /// Says whether the node took the step: not while it is down, nor when
    /// the step's commit failed and stopped it.
    pub fn step(&mut self, index: usize, f: impl FnOnce(&mut Core)) -> bool {
        self.touched.insert(index);
        let now = self.local_now(index);
        let node = &mut self.nodes[index];
        let failing = node.failing.clone();
        let Some(core) = node.core.as_mut() else {
            return false;
        };
        let mut outputs = Vec::new();
        let step = || {
            core.begin_step(now);
            f(core);
            core.finish_step(&mut outputs);
        };
        match failing {
            None => step(),
            Some(failed) => {
                if let Err(panic) = catching(step) {
                    // Only the commit made to fail stops the node; any
                    // other panic fails the run.
                    let message = panic.downcast_ref::<String>().map(String::as_str);
                    let held = failed.held().filter(|_| {
                        message.is_some_and(|message| message.starts_with(COMMIT_FAILED))
                    });
                    let Some(held) = held else {
                        resume_unwind(panic);
                    };
                    self.commit_failed(index, held);
                    return false;
                }
            }
        }
        if self.settling.across() {
            self.watch_prices(index);
        }
        if matches!(
            self.settling,
            Settling::StubHub(_)
                | Settling::HubDsusd(_)
                | Settling::Hub(_)
                | Settling::HubAcross(_)
                | Settling::Hubs(_)
        ) {
            self.watch_hub(index);
        }
        if self.settling.on_xrpl() {
            self.watch_proofs(index);
        }
        for output in outputs {
            self.carry(index, output);
        }
        true
    }

    /// Checks node `index`'s peerings over the stub with commitments after
    /// a step. The stub holds no more of its packets to a peer, or of the
    /// peer's to it, than the connector holds pending with that peer: a
    /// commitment the stub still held for a packet the connector let go
    /// would never be resolved, and would take a place the stub has for the
    /// peer for good, as a hub's member would keep a promise for a packet
    /// nobody answers. And the credit the node extends to each peer is its
    /// hold toward the peer and its settlements to it still on their way
    /// (spec/areas/hubs.md#in-the-node). A run keeps the first it finds of
    /// each node and peer, and of the first few.
    fn watch_hub(&mut self, index: usize) {
        let Some(core) = self.nodes[index].core.as_mut() else {
            return;
        };
        let peers = core.hub_peers();
        // Each peer's holds, summed over the methods of its peering, each
        // over a hub of its own: what it can cost us is bounded by all of
        // them together (spec/decisions/settlement-methods.md).
        let mut holds_with: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for peer in &peers {
            let sum = holds_with.entry(peer.peer.to_string()).or_default();
            sum.0 = sum.0.saturating_add(peer.holds.toward);
            sum.1 = sum.1.saturating_add(peer.holds.from);
        }
        for (peer, (toward, from)) in holds_with {
            let most = self.hub_most_held.entry((index, peer)).or_default();
            most.0 = most.0.max(toward);
            most.1 = most.1.max(from);
        }
        for peer in peers {
            let held = peer.held;
            if held.ours > peer.pending_to || held.theirs > peer.pending_from {
                let leak = format!(
                    "{} node {index}'s stub holds {} of its packets to {} and {} of that \
                     peer's to it, while its connector holds {} and {} pending",
                    self.now, held.ours, peer.peer, held.theirs, peer.pending_to, peer.pending_from,
                );
                record_once(&mut self.hub_leaks, index, peer.peer, leak);
            }
            let holds = peer.holds.toward.saturating_add(peer.settling);
            // Over a real hub, a take from our hold reaches the books only
            // once the hub reports it, and the credit is lowered only as far
            // as what the peer may still owe us allows: it may stay above
            // the holds for a while, never below them
            // (spec/areas/hubs.md#in-the-node). The stub reports a take when
            // the books take it, so there the two are always one.
            let real = !matches!(self.settling, Settling::StubHub(_));
            if peer.peered
                && let Some(credit) = peer.credit
                && (credit < holds || credit > holds && !real)
            {
                let off = format!(
                    "{} node {index}'s stub extends {} credit of {credit}, while its hold \
                     toward it is {} and its settlements to it on their way {}",
                    self.now, peer.peer, peer.holds.toward, peer.settling,
                );
                record_once(&mut self.hub_credit_off, index, peer.peer, off);
            }
        }
    }

    /// Counts what node `index`'s XRPL accounts proved, and the lies they
    /// caught, since its last step: so a run shows that it reached the
    /// verifier, and a change that quietly bypasses it fails its coverage
    /// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
    fn watch_proofs(&mut self, index: usize) {
        let node = &self.nodes[index];
        let Some(core) = node.core.as_ref() else {
            return;
        };
        let mut proven = Proofs::default();
        let mut caught = 0u64;
        for account in core.xrpl_accounts() {
            let more = account.proofs();
            proven.anchors += more.anchors;
            proven.transactions += more.transactions;
            proven.absences += more.absences;
            proven.never_applied += more.never_applied;
            caught += Doubt::ALL
                .iter()
                .map(|doubt| account.doubts(*doubt))
                .sum::<u64>();
        }
        let generation = node.generation;
        let (before, caught_before) = match self.proofs_seen.get(&index) {
            Some(&(seen, before, caught)) if seen == generation => (before, caught),
            _ => (Proofs::default(), 0),
        };
        self.proofs_seen.insert(index, (generation, proven, caught));
        for (what, then, now) in [
            ("anchors", before.anchors, proven.anchors),
            (
                "transactions proven",
                before.transactions,
                proven.transactions,
            ),
            ("absences proven", before.absences, proven.absences),
            (
                "transactions proven never to apply",
                before.never_applied,
                proven.never_applied,
            ),
            ("lies caught", caught_before, caught),
        ] {
            if now > then {
                *self.counts.entry(what).or_default() += now - then;
            }
        }
    }

    /// Counts, as a run ends, the lies the fake ledger's servers told, by
    /// kind, and the nodes running on it whose account anchored.
    pub fn count_xrpl(&mut self) {
        let anchored = (self.nodes.iter())
            .filter_map(|node| node.core.as_ref())
            .filter(|core| {
                let mut accounts = core.xrpl_accounts().peekable();
                accounts.peek().is_some() && accounts.all(|account| account.anchor().is_some())
            })
            .count();
        if anchored > 0 {
            *self.counts.entry("XRPL nodes anchored").or_default() +=
                u64::try_from(anchored).unwrap();
        }
        for lie in Lie::ALL {
            let told = self.ledger.lies_told(lie);
            if told > 0 {
                *self.counts.entry(told_as(lie)).or_default() += told;
                *self.counts.entry("lies told").or_default() += told;
            }
        }
    }

    /// Checks node `index`'s prices after a step against its own rates and
    /// books (spec/decisions/passive-rebalancing.md): none above spot by
    /// more than its `k`, less its spread; none that takes anything out of
    /// a ledger its owner holds nothing on; and none better than at what is
    /// posted alone, since no packet in flight may improve a price.
    fn watch_prices(&mut self, index: usize) {
        let Some(core) = self.nodes[index].core.as_ref() else {
            return;
        };
        let table = *core.price_table();
        let pricing = table.pricing();
        let million = 1_000_000u128;
        let mut breaches = Vec::new();
        let mut posted = table.holdings();
        for holding in &mut posted {
            holding.debits_pending = 0;
            holding.credits_pending = 0;
        }
        for price in table.prices() {
            match table.rate(price.from, price.to, 0, 0, &posted) {
                Ok(rate_e15) if rate_e15 >= price.rate_e15 => {}
                at_posted => {
                    breaches.push(format!(
                        "{price:?} is above {at_posted:?} at what is posted"
                    ));
                }
            }
            let rate = |ledger: LedgerId| core.rates().get(ledger.asset().code).unwrap();
            // rate ≤ spot · (1 + k) · (1 − spread) · 10^15, all scale 9,
            // within the unit the bound loses rounding down twice.
            let bonus = rate(price.to)
                * 1_000_000_000_000_000
                * (million + u128::from(pricing.rebalance_ppm))
                / rate(price.from);
            let most =
                bonus * (million - u128::from(pricing.charges.spread_ppm)) / (million * million);
            if u128::from(price.rate_e15) > most + 1 {
                breaches.push(format!("{price:?} is above {most}"));
            }
            if core.owner_available(price.to) == 0 {
                breaches.push(format!("{price:?} takes out of an empty ledger"));
            }
        }
        self.count("price tables checked");
        for breach in breaches {
            self.priced
                .push(format!("{} node {index}: {breach}", self.now));
        }
    }

    /// Keeps what a node reported in the trace, which a replay compares,
    /// and an error or a lost event as a failure of the run
    /// (spec/decisions/tigerstyle.md).
    pub fn record(&mut self, index: usize, event: &Event) {
        if matches!(
            event,
            Event::Settlement(SettlementEvent::PeerSettled { .. })
        ) {
            self.count("settlements taken");
        }
        if let Event::Settlement(SettlementEvent::PeerLoss { peer, amount, .. }) = event {
            self.count("losses booked over a hub");
            *self
                .hub_losses
                .entry((index, peer.to_string()))
                .or_default() += *amount;
        }
        if let Event::PeerNode(dassie_peer::Event::Depeering { reason, .. }) = event {
            match reason {
                dassie_peer::wire::TerminationReason::Unresponsive => {
                    self.count("peerings ended for silence")
                }
                dassie_peer::wire::TerminationReason::ShortOfRoom => {
                    self.count("peerings ended for room")
                }
                _ => {}
            }
        }
        self.trace.push(format!("{} {index} {event:?}", self.now));
        if let Event::PeerNode(dassie_peer::Event::Advertised {
            peer, short: true, ..
        }) = event
        {
            let scheme = self.nodes[index].core.as_ref().and_then(|core| {
                core.network()
                    .peers
                    .into_iter()
                    .find(|view| view.id == *peer)
                    .map(|view| view.peering.first().scheme)
            });
            self.withdrawn.push(Withdrawal {
                at: self.now,
                by: index,
                peer: *peer,
                scheme,
            });
        }
        if let Event::Settlement(SettlementEvent::Reconciled {
            cause: dassie_settlement::ReconcileCause::Unexplained,
            ..
        }) = event
        {
            self.unexplained
                .push(format!("{} node {index}: {event:?}", self.now));
        }
        let lost = matches!(event, Event::Engine(EngineEvent::EventsDropped { .. }));
        if event.severity() == Severity::Error || lost {
            self.errors
                .push(format!("{} node {index}: {event:?}", self.now));
        }
    }

    fn carry(&mut self, index: usize, output: Output) {
        match output {
            Output::PeerRequest(request) => {
                let to = self.nodes.len();
                let to = (0..to)
                    .find(|i| request.endpoint == Some(endpoint(*i)))
                    .unwrap();
                let generation = self.nodes[index].generation;
                self.watch_sent(index, to, request.id, &request.body);
                // When a node asks a peer to settle, so that the checks can
                // tell how long it waited for the answer
                // (spec/decisions/ending-a-peering.md).
                match dassie_peer::wire::peek_kind(&request.body) {
                    Some(dassie_peer::wire::Kind::SettlementRequest) => {
                        self.trace
                            .push(format!("{} {index} asks {to} to settle", self.now));
                    }
                    Some(dassie_peer::wire::Kind::MethodPacket) if hub_prepare(&request.body) => {
                        self.count("prepares over a hub");
                    }
                    _ => {}
                }
                let withheld = self.hub_intercept(index, to, &request.body);
                if withheld
                    || self.rng.next_u64() % 1000 < self.loss_per_mille
                    || self.cut.contains(&(index, to))
                    || self.apart.contains(&index) != self.apart.contains(&to)
                {
                    let delay = self.latency();
                    let result = Err(Failure::NotDelivered);
                    self.schedule(
                        delay,
                        Delivery::Result {
                            to: index,
                            generation,
                            id: request.id,
                            result,
                        },
                    );
                    return;
                }
                let delay = self.latency();
                let body = request.body;
                self.schedule(
                    delay,
                    Delivery::Request {
                        from: index,
                        generation,
                        to,
                        id: request.id,
                        body,
                    },
                );
            }
            Output::PeerReply { inbound, reply } => {
                if let Some(kind) = self.nodes[index].forged.remove(&inbound) {
                    self.forgery_answered(index, kind, reply.status);
                    return;
                }
                if let Some(key) = self.nodes[index].fulfills.remove(&inbound)
                    && reply.status == 200
                {
                    self.took_fulfill(index, key);
                }
                let Some((to, generation, id)) = self.nodes[index].inbound.remove(&inbound) else {
                    return;
                };
                let delay = self.latency();
                let result = Ok(Response {
                    status: reply.status,
                    body: reply.body,
                });
                self.schedule(
                    delay,
                    Delivery::Result {
                        to,
                        generation,
                        id,
                        result,
                    },
                );
            }
            Output::Btp { link, message } => self.deliver_to_client(index, link, message),
            Output::Event(event) => self.record(index, &event),
            Output::Trace(_) | Output::ProbeDone { .. } => {}
            Output::LedgerConnect { epoch, .. } => self.ledger_connect(index, epoch),
            Output::LedgerClose { epoch, .. } => {
                if self
                    .links
                    .get(&index)
                    .is_some_and(|link| link.epoch == epoch)
                    && let Some(link) = self.links.remove(&index)
                {
                    self.ledger.disconnect(link.connection);
                }
            }
            Output::LedgerSend { epoch, message, .. } => {
                let Some(link) = self.links.get(&index).filter(|link| link.epoch == epoch) else {
                    return;
                };
                if self.ledger_down.contains(&index) {
                    return;
                }
                let connection = link.connection;
                // A request may be lost; the scheme hears nothing, and
                // connects anew.
                if self.rng.next_u64() % 1000 < self.loss_per_mille {
                    return;
                }
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerRequest {
                        from: index,
                        connection,
                        message,
                    },
                );
            }
            Output::HubSend {
                asset,
                epoch,
                hub,
                endpoint,
                message,
                ..
            } => self.hub_send(index, &hub, asset, epoch, endpoint, message),
            Output::Fetch { epoch, url, .. } => {
                self.count("validator lists fetched");
                let body = self.ledger.fetch(&url);
                let generation = self.nodes[index].generation;
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerEvent {
                        to: index,
                        generation,
                        epoch,
                        event: LedgerEvent::Fetched { url, body },
                    },
                );
            }
            Output::Fund { epoch, body, .. } => {
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let to = dassie_xrpl::AccountId::parse(request["destination"].as_str().unwrap())
                    .unwrap();
                self.ledger.fund(to);
                *self.fundings.entry(index).or_default() += 1;
                let generation = self.nodes[index].generation;
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerEvent {
                        to: index,
                        generation,
                        epoch,
                        event: LedgerEvent::Funded(true),
                    },
                );
            }
        }
    }

    /// Opens a node's connection to the fake ledger.
    fn ledger_connect(&mut self, index: usize, epoch: u64) {
        if let Some(link) = self.links.remove(&index) {
            self.ledger.disconnect(link.connection);
        }
        self.next_connection += 1;
        let connection = self.next_connection;
        self.links.insert(index, LedgerLink { epoch, connection });
        let generation = self.nodes[index].generation;
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::LedgerEvent {
                to: index,
                generation,
                epoch,
                event: LedgerEvent::Connected,
            },
        );
    }

    /// The token the run's issuer issues: its network's dollar unless the
    /// run is on another.
    fn token(&self) -> IssuedAsset {
        self.settling.token().map_or(
            if self.settling.network().faucet.is_some() {
                IssuedAsset::DSUSD
            } else {
                IssuedAsset::RLUSD
            },
            |(_, token)| token,
        )
    }

    /// Whether the run's nodes need rates: to convert across currencies,
    /// or for a token's peg guard.
    const fn needs_rates(&self) -> bool {
        self.settling.across() || self.settling.token().is_some()
    }

    /// Every running node learns [`RATES`] again, once a minute, as a node
    /// fetches them, in a run that needs them.
    fn learn_rates(&mut self) {
        if !self.needs_rates() || self.now < self.next_rates {
            return;
        }
        self.next_rates = self.now + Policies::DEFAULT.rates_every_ms;
        for index in 0..self.nodes.len() {
            let now = self.local_now(index);
            let rates = RATES.map(|(c, r)| (c, r, now));
            self.step(index, |core| core.set_rates(&rates));
        }
    }

    /// The faucet funds node `index` once more, in the next ledger, as its
    /// owner may ask it: a deposit of the owner's.
    pub fn fund(&mut self, index: usize) {
        self.ledger.fund(self.xrpl_account(index));
        *self.fundings.entry(index).or_default() += 1;
        self.trace
            .push(format!("{} the faucet funds {index}", self.now));
    }

    /// The issuer mints [`MINT_MICROS`] of its token to each node it has
    /// not yet minted to whose line exists, in the ledger about to close.
    fn mint(&mut self) {
        let lined: Vec<usize> = self
            .to_mint
            .iter()
            .copied()
            .filter(|index| self.line(*index).is_some())
            .collect();
        let token = self.token();
        for index in lined {
            let value = TokenValue::from_units(MINT_MICROS, 6).unwrap();
            self.ledger
                .issue(self.xrpl_account(index), token.amount(value))
                .unwrap();
            self.to_mint.remove(&index);
            self.issued.entry(index).or_default().0 += MINT_MICROS as i128 * 1_000;
            self.count("issuer mints");
            self.trace.push(format!(
                "{} the issuer mints {index} {}",
                self.now,
                token.currency.code()
            ));
        }
    }

    /// What node `index`'s line holds of the run's token, if it has one, in
    /// its ledger's internal units: whole millionths, rounded down.
    pub fn line(&self, index: usize) -> Option<i128> {
        let held = self
            .ledger
            .token_balance(self.xrpl_account(index), self.token())?;
        Some(held.floor_units(6)? * 1_000)
    }

    /// The issuer claws `units` of its token back from node `index`, in the next
    /// ledger: well within its line, so all of it.
    pub fn claw_back(&mut self, index: usize, units: i128) {
        let line = self.line(index).unwrap_or(0);
        assert!(units > 0 && units * 2 <= line, "a clawback within the line");
        let micros = u64::try_from(units / 1_000).unwrap();
        let value = TokenValue::from_units(micros, 6).unwrap();
        let token = self.token();
        self.ledger
            .claw_back(self.xrpl_account(index), token.amount(value))
            .unwrap();
        self.issued.entry(index).or_default().1 += units;
        self.count("issuer clawbacks");
        self.trace.push(format!(
            "{} the issuer claws back {units} of {index}'s {}",
            self.now,
            token.currency.code()
        ));
    }

    /// The issuer freezes, in the next ledger, or lifts what it froze.
    pub fn issuer_freeze(&mut self, freeze: IssuerFreeze, on: bool) {
        let token = self.token();
        match freeze {
            IssuerFreeze::Line(index) => self.ledger.freeze(self.xrpl_account(index), token, on),
            IssuerFreeze::Deep(index) if on => {
                self.ledger
                    .deep_freeze(self.xrpl_account(index), token, true)
            }
            // Lifted whole, the freeze with the deep freeze.
            IssuerFreeze::Deep(index) => self.ledger.freeze(self.xrpl_account(index), token, false),
            IssuerFreeze::Global => self.ledger.global_freeze(token.issuer, on),
        }
        .unwrap();
        // It applies with the next ledger, and is lifted with it.
        let at = self.next_close;
        if on {
            self.freezes.push((freeze, at, None));
            self.count(match freeze {
                IssuerFreeze::Line(_) => "issuer freezes",
                IssuerFreeze::Deep(_) => "issuer deep freezes",
                IssuerFreeze::Global => "issuer global freezes",
            });
        } else if let Some(held) = self
            .freezes
            .iter_mut()
            .rev()
            .find(|(held, _, until)| *held == freeze && until.is_none())
        {
            held.2 = Some(at);
        }
        self.trace.push(format!(
            "{} the issuer {} {freeze:?}",
            self.now,
            if on { "freezes" } else { "lifts" }
        ));
    }

    /// A node's connection to the ledger drops; it comes back after a
    /// while, as the driver's would. Returns whether it did: not while the
    /// node has no connection, nor when the step failed its commit.
    pub fn drop_ledger_link(&mut self, index: usize) -> bool {
        let Some(link) = self.links.get(&index).copied() else {
            return false;
        };
        self.ledger.disconnect(link.connection);
        let generation = self.nodes[index].generation;
        let asset = self.xrpl_asset();
        let stepped = self.step(index, |core| {
            core.ledger_event(&asset, link.epoch, &LedgerEvent::Disconnected);
        });
        if !stepped {
            return false;
        }
        self.trace
            .push(format!("{} ledger link {index} drops", self.now));
        self.next_connection += 1;
        let connection = self.next_connection;
        self.links.insert(
            index,
            LedgerLink {
                epoch: link.epoch,
                connection,
            },
        );
        self.schedule(
            LEDGER_RECONNECT_MS,
            Delivery::LedgerEvent {
                to: index,
                generation,
                epoch: link.epoch,
                event: LedgerEvent::Connected,
            },
        );
        true
    }

    /// A node's connection to the ledger drops, and comes back to a server
    /// that tells `lie` for `for_ms`; then it drops again, and the next is
    /// honest. The node may leave it sooner, for a lie it caught or a
    /// request it never got answered: its next connection is honest too
    /// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
    pub fn lie_to(&mut self, index: usize, lie: Lie, for_ms: u64) {
        if !self.drop_ledger_link(index) {
            return;
        }
        let connection = self.links[&index].connection;
        self.ledger.set_lie(connection, Some(lie));
        self.lying.insert(index, connection);
        self.count("lying servers");
        self.trace.push(format!(
            "{} ledger link {index} lies for {} s: {lie:?}",
            self.now,
            for_ms / 1_000
        ));
        self.schedule(
            LEDGER_RECONNECT_MS + for_ms,
            Delivery::Honest {
                to: index,
                connection,
            },
        );
    }

    /// Every connection to a lying server ends now: the next is honest.
    pub fn end_lies(&mut self) {
        for (index, connection) in std::mem::take(&mut self.lying) {
            self.honest(index, connection);
        }
    }

    /// Node `index`'s connection to a lying server, `connection`, ends if
    /// it is still open.
    fn honest(&mut self, index: usize, connection: u64) {
        if self.lying.get(&index) == Some(&connection) {
            self.lying.remove(&index);
        }
        if self
            .links
            .get(&index)
            .is_some_and(|link| link.connection == connection)
        {
            self.ledger.set_lie(connection, None);
            self.drop_ledger_link(index);
        }
    }

    /// The fake ledger closes a ledger, and its streams go out. The issuer
    /// mints first to each node whose line has come to exist.
    fn close_ledger(&mut self) {
        self.next_close = self.now + LEDGER_CLOSE_MS;
        self.learn_rates();
        self.mint();
        self.submit_payout();
        let close_time = dassie_xrpl::unl::ledger_seconds(self.now);
        let closed = self.ledger.close_at(close_time);
        self.payout_closed();
        for (connection, message) in closed {
            if self.validator_stream(connection, message.clone()) {
                continue;
            }
            let Some((&to, link)) = self
                .links
                .iter()
                .find(|(_, link)| link.connection == connection)
            else {
                continue;
            };
            let (epoch, generation) = (link.epoch, self.nodes[to].generation);
            let delay = self.latency();
            self.schedule(
                delay,
                Delivery::LedgerEvent {
                    to,
                    generation,
                    epoch,
                    event: LedgerEvent::Message(message),
                },
            );
        }
    }

    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Request {
                from,
                generation,
                to,
                id,
                body,
            } => {
                if self.nodes[to].core.is_none() {
                    let delay = self.latency();
                    let result = Err(Failure::NotDelivered);
                    self.schedule(
                        delay,
                        Delivery::Result {
                            to: from,
                            generation,
                            id,
                            result,
                        },
                    );
                    return;
                }
                // Someone who saw it arrive sends it again, later, as its
                // own: a request that never arrived was never sent, so no
                // one has it. A run without forgeries draws nothing for
                // them.
                if self.forgery_per_mille > 0 && self.rng.next_u64() % 1000 < self.forgery_per_mille
                {
                    let later = 1 + self.rng.next_u64() % FORGERY_WITHIN_MS;
                    let forged = Delivery::Forged {
                        from,
                        to,
                        arrived: self.now,
                        body: body.clone(),
                    };
                    self.schedule(later, forged);
                }
                let fulfill = match ilp_packet(&body) {
                    Some((request_id, dassie_ilp::Packet::Fulfill(fulfill))) => {
                        let condition = dassie_ilp::condition_for(&fulfill.fulfillment);
                        Some((to, from, request_id, condition))
                    }
                    _ => None,
                };
                let fulfill = fulfill.filter(|key| self.sent.contains_key(key));
                let node = &mut self.nodes[to];
                let inbound = InboundId(node.next_inbound);
                node.next_inbound += 1;
                node.inbound.insert(inbound, (from, generation, id));
                if let Some(key) = fulfill {
                    node.fulfills.insert(inbound, key);
                }
                let sender = key(from).public_key();
                self.step(to, |core| core.peer_inbound(inbound, sender, &body, None));
            }
            Delivery::Forged {
                from,
                to,
                arrived,
                body,
            } => {
                if self.nodes[to].core.is_none() {
                    return;
                }
                self.count("forgeries");
                self.trace.push(format!(
                    "{} forgery to {to} of {from}'s request that arrived at {arrived}",
                    self.now
                ));
                let kind = dassie_peer::wire::peek_kind(&body);
                let node = &mut self.nodes[to];
                let inbound = InboundId(node.next_inbound);
                node.next_inbound += 1;
                node.forged.insert(inbound, kind);
                let sender = forger().public_key();
                self.step(to, |core| core.peer_inbound(inbound, sender, &body, None));
            }
            Delivery::LedgerRequest {
                from,
                connection,
                message,
            } => {
                let Some(link) = self.links.get(&from).copied() else {
                    return;
                };
                if link.connection != connection {
                    return;
                }
                let reply = self.ledger.request(connection, &message);
                let generation = self.nodes[from].generation;
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerEvent {
                        to: from,
                        generation,
                        epoch: link.epoch,
                        event: LedgerEvent::Message(reply),
                    },
                );
            }
            Delivery::LedgerEvent {
                to,
                generation,
                epoch,
                event,
            } => {
                if self.nodes[to].generation == generation {
                    let asset = self.xrpl_asset();
                    self.step(to, |core| core.ledger_event(&asset, epoch, &event));
                }
            }
            Delivery::HubRequest {
                member,
                generation,
                asset,
                epoch,
                endpoint,
                message,
            } => self.hub_request(member, generation, asset, epoch, endpoint, message),
            Delivery::HubAnswer {
                member,
                generation,
                asset,
                epoch,
                endpoint,
                message,
            } => self.hub_answer(member, generation, &asset, epoch, endpoint, message),
            Delivery::HubExchangeEnd { member, endpoint } => {
                self.hub_exchange_end(member, endpoint);
            }
            Delivery::HubSubmit {
                member,
                endpoint,
                message,
            } => self.hub_submit(member, endpoint, message),
            Delivery::ValidatorMessage {
                from,
                to,
                message,
                reply,
            } => self.validator_message(from, to, message, reply),
            Delivery::ValidatorLedgerRequest {
                validator,
                connection,
                message,
            } => self.validator_ledger_request(validator, connection, &message),
            Delivery::ValidatorLedgerEvent {
                validator,
                generation,
                connection,
                message,
            } => self.validator_ledger_event(validator, generation, connection, message),
            Delivery::ValidatorState {
                validator,
                generation,
                finalized,
                state,
            } => self.validator_state(validator, generation, finalized, state),
            Delivery::Honest { to, connection } => self.honest(to, connection),
            Delivery::Result {
                to,
                generation,
                id,
                result,
            } => {
                if let Some(key) = self.fulfills_sent.remove(&(to, generation, id))
                    && matches!(&result, Ok(response) if matches!(response.status, 400 | 401 | 409))
                    && let Some(sent) = self.sent.get_mut(&key)
                {
                    Lost::record(&mut sent.lost, Lost::Refused);
                }
                if self.nodes[to].generation == generation {
                    self.step(to, |core| core.peer_response(id, result));
                }
            }
        }
    }

    /// Follows an ILP packet node `from` sends node `to`: a Prepare until
    /// it is known whether its Fulfill reached `from`'s books, and the
    /// Fulfill or Reject `from` answers one of `to`'s with. It watches
    /// what crosses the network, never the books
    /// (spec/decisions/own-books.md).
    fn watch_sent(&mut self, from: usize, to: usize, id: RequestId, body: &[u8]) {
        let Some((request_id, packet)) = ilp_packet(body) else {
            return;
        };
        match packet {
            dassie_ilp::Packet::Prepare(prepare) if prepare.amount > 0 => {
                let key = (from, to, request_id, prepare.execution_condition);
                let expires_at_ms = prepare.expires_at.unix_ms;
                // The sender takes a Fulfill while its clock is before the
                // expiry; the peer books one while its own is, and sends it
                // again for as long as a request may take.
                let taken_by = expires_at_ms.saturating_add_signed(-self.nodes[from].skew_ms);
                let sent_by = expires_at_ms
                    .saturating_add_signed(-self.nodes[to].skew_ms)
                    .saturating_add(self.peer.request_timeout_ms);
                let at = taken_by.max(sent_by).max(self.now) + 1;
                let sent = Sent {
                    amount: prepare.amount,
                    expires_at_ms,
                    generation: self.nodes[from].generation,
                    fulfilled: false,
                    lost: None,
                };
                if self.sent.insert(key, sent).is_none() {
                    self.finals.insert((at, key));
                }
                assert!(self.sent.len() <= MAX_SENT, "too many Prepares to follow");
                assert!(self.finals.len() <= MAX_SENT, "too many Prepares to follow");
            }
            dassie_ilp::Packet::Fulfill(fulfill) => {
                let condition = dassie_ilp::condition_for(&fulfill.fulfillment);
                let key = (to, from, request_id, condition);
                if let Some(sent) = self.sent.get_mut(&key) {
                    sent.fulfilled = true;
                    let generation = self.nodes[from].generation;
                    self.fulfills_sent.insert((from, generation, id), key);
                    assert!(
                        self.fulfills_sent.len() <= MAX_SENT,
                        "too many Fulfills to follow"
                    );
                }
            }
            dassie_ilp::Packet::Reject(_) => {
                // Refused, it was booked by neither.
                let first = (to, from, request_id, [0; 32]);
                let last = (to, from, request_id, [u8::MAX; 32]);
                let refused: Vec<PacketKey> =
                    self.sent.range(first..=last).map(|(key, _)| *key).collect();
                for key in refused {
                    self.sent.remove(&key);
                }
            }
            dassie_ilp::Packet::Prepare(_) => {}
        }
    }

    /// Node `index` answered 200 to a Fulfill of the Prepare `key`: its
    /// connector took it if it still had the packet, before its expiry, and
    /// otherwise it came late.
    fn took_fulfill(&mut self, index: usize, key: PacketKey) {
        let now = self.local_now(index);
        let generation = self.nodes[index].generation;
        let Some(sent) = self.sent.get_mut(&key) else {
            return;
        };
        if sent.generation == generation && now < sent.expires_at_ms {
            self.sent.remove(&key);
        } else {
            Lost::record(&mut sent.lost, Lost::Late);
        }
    }

    /// Books, as drift, each Prepare past being taken whose Fulfill its
    /// peer booked and its sender never took.
    fn finalise_sent(&mut self) {
        while let Some(&(at, key)) = self.finals.first() {
            if at > self.now {
                return;
            }
            self.finals.pop_first();
            let Some(sent) = self.sent.remove(&key) else {
                continue;
            };
            if !sent.fulfilled {
                continue;
            }
            // A request whose result never came, as when its recipient
            // crashed answering it, goes with its Prepare.
            self.fulfills_sent.retain(|_, sent| *sent != key);
            let (from, to, ..) = key;
            let drift = self.drift.entry((from.min(to), from.max(to))).or_default();
            let amount = u128::from(sent.amount);
            let (how, why) = match sent.lost {
                Some(Lost::SenderCrash | Lost::PeerCrash) => {
                    drift.crashes += amount;
                    ("fulfills lost to crashes", "a crash")
                }
                Some(Lost::Refused) => {
                    drift.others += amount;
                    ("fulfills refused", "refused")
                }
                Some(Lost::Late) | None => {
                    drift.others += amount;
                    ("fulfills lost late", "late")
                }
            };
            self.count(how);
            self.trace.push(format!(
                "{} {to}'s Fulfill of {amount} never reached {from}'s books: {why}",
                self.now
            ));
        }
    }

    /// How far the books of nodes `a` and `b` drifted, by what really
    /// happened: the Fulfills one booked and the other never did
    /// (spec/decisions/crash-drift.md). It counts only what surely drifted,
    /// so it may fall short of the drift, and never exceeds it.
    pub fn drift(&mut self, a: usize, b: usize) -> Drift {
        self.finalise_sent();
        self.drift
            .get(&(a.min(b), a.max(b)))
            .copied()
            .unwrap_or_default()
    }

    /// Runs until `until`, also steering the payments.
    pub fn run_until(&mut self, until: u64) {
        self.run_until_or(until, |_| false);
    }

    /// Runs until `until`, or until `stop` holds after an event; returns
    /// whether it stopped.
    pub fn run_until_or(&mut self, until: u64, stop: impl Fn(&Self) -> bool) -> bool {
        // Each node's deadline, on its own clock. A core's deadline changes
        // only in a step, and every step, start and stop marks its node, so
        // only the nodes marked since are asked again: asking every node
        // before every step cost much of a large network's run.
        let mut deadlines: Vec<Option<u64>> = (0..self.nodes.len())
            .map(|index| self.local_deadline(index))
            .collect();
        self.touched.clear();
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(
                steps < 5_000_000,
                "no progress at {}: {:?}, queue {:?}, hub {:?}, nodes {:?}",
                self.now,
                self.counts,
                self.queue.keys().next(),
                self.hub_deadline(),
                (0..self.nodes.len())
                    .map(|i| self.deadline(i))
                    .collect::<Vec<_>>()
            );
            self.finalise_sent();
            if stop(self) {
                return true;
            }
            self.drive_payments();
            self.refresh(&mut deadlines);
            let delivery = self.queue.keys().next().map(|(at, _)| *at);
            let timer = deadlines
                .iter()
                .zip(&self.nodes)
                // Back from the node's clock to the network's.
                .filter_map(|(local, node)| {
                    Some(local.as_ref()?.saturating_add_signed(-node.skew_ms))
                })
                .chain(self.hub_deadline())
                .min();
            let earliest = [delivery, timer].into_iter().flatten().min();
            let close = self.settling.on_xrpl().then_some(self.next_close);
            if close.is_some_and(|at| at <= until && earliest.is_none_or(|e| at <= e)) {
                self.now = self.now.max(self.next_close);
                self.close_ledger();
                continue;
            }
            let Some(next) = [delivery, timer]
                .into_iter()
                .flatten()
                .min()
                .filter(|at| *at <= until)
            else {
                self.now = until;
                return false;
            };
            self.now = self.now.max(next);
            if delivery == Some(next) {
                let key = *self.queue.keys().next().unwrap();
                let delivery = self.queue.remove(&key).unwrap();
                self.deliver(delivery);
                continue;
            }
            for index in 0..self.nodes.len() {
                self.refresh(&mut deadlines);
                let due = deadlines[index]
                    .map(|local| local.saturating_add_signed(-self.nodes[index].skew_ms));
                if due.is_some_and(|at| at <= self.now) {
                    self.step(index, |_| {});
                }
            }
            self.hub_due();
        }
    }

    /// Starts a payment of `amount` from a BTP client of `from` to the
    /// receiver of `to`. Says whether it started: not when a failed commit
    /// stopped either node before its part was on disk.
    pub fn pay(
        &mut self,
        from: usize,
        to: usize,
        amount: u64,
        cap: Option<SpendingCap>,
        limit: Option<u64>,
    ) -> bool {
        // Paid from the sender's first ledger, into the receiver's last.
        let ledger = self.settling.ledgers(from)[0];
        let into = *self.settling.ledgers(to).last().unwrap();
        self.pay_in(from, (ledger, into), to, amount, cap, limit)
    }

    /// As [`Sim::pay`], from the sender's `ledgers.0` into the receiver's
    /// `ledgers.1`.
    pub fn pay_in(
        &mut self,
        from: usize,
        (ledger, into): (LedgerId, LedgerId),
        to: usize,
        amount: u64,
        cap: Option<SpendingCap>,
        limit: Option<u64>,
    ) -> bool {
        let mut credentials = None;
        let created = self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: into,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: limit,
                    expires_at_ms: None,
                    metadata: None,
                    uplink: None,
                })
                .unwrap();
            credentials = Some((payment.id, payment.ilp_address, payment.shared_secret));
        });
        if !created {
            return false;
        }
        let (incoming, destination, shared_secret) = credentials.unwrap();
        let mut session = None;
        let connected = self.step(from, |core| {
            let token = core
                .create_token("sim".into(), ledger.as_str(), cap)
                .unwrap();
            session = Some((token.view.id, core.btp_connect(&token.secret).unwrap()));
        });
        if !connected {
            return false;
        }
        let (token, session) = session.unwrap();
        let credentials = (session.address.clone(), destination, shared_secret);
        let connection = client(&credentials, ledger);
        self.payments.push(Payment {
            from,
            to,
            incoming,
            limit,
            token: (token, cap),
            ledger,
            link: session.link,
            connection,
            credentials,
            sent_before: 0,
            amount,
            stream: None,
            in_flight: BTreeMap::new(),
            next_request: 1,
            done: false,
            expired: false,
            rate_retry_at: None,
        });
        self.trace
            .push(format!("{} pay {from} -> {to} {amount}", self.now));
        true
    }

    /// An outgoing payment of `amount` from node `from` to an incoming
    /// payment of node `to`, which may be the same node.
    ///
    /// # Errors
    ///
    /// When node `from` refuses it, as it does while its owner has too
    /// little to pay it: on the XRP Ledger, before the faucet funded it.
    /// Or when a failed commit stopped either node before its part was on
    /// disk.
    pub fn pay_outgoing(&mut self, from: usize, to: usize, amount: u64) -> Result<(), String> {
        let ledger = self.settling.ledger();
        let mut incoming = None;
        let stopped = |index: usize| format!("node {index} stopped");
        let created = self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: Some(amount),
                    expires_at_ms: None,
                    metadata: None,
                    uplink: None,
                })
                .unwrap();
            incoming = Some(payment);
        });
        if !created {
            return Err(stopped(to));
        }
        let incoming = incoming.unwrap();
        let mut outgoing = None;
        let sent = self.step(from, |core| {
            let created = core.create_outgoing_payment(NewOutgoing {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                grant: None,
                quote_id: None,
                receiver: format!("https://n{to}.test/incoming-payments/{}", incoming.id),
                destination: incoming.ilp_address.clone(),
                shared_secret: incoming.shared_secret,
                debit_amount: amount,
                receive_amount: ForeignAmount {
                    value: amount,
                    asset_code: ledger.asset().code.into(),
                    asset_scale: 9,
                },
                metadata: None,
                recipient: Some(format!("https://n{to}.test/.well-known/pay")),
                fixed_debit: false,
            });
            outgoing = Some(created.map(|created| created.row.id));
        });
        if !sent {
            return Err(stopped(from));
        }
        let outgoing = outgoing.unwrap().map_err(|error| format!("{error:?}"))?;
        self.outgoing.push((from, to, incoming.id, outgoing));
        self.trace
            .push(format!("{} outgoing {from} -> {to} {amount}", self.now));
        Ok(())
    }

    /// Lets every payment send what it can.
    fn drive_payments(&mut self) {
        let now = Timestamp::from_unix_ms(self.now);
        let mut buffer = vec![0u8; BUFFER_LENGTH];
        let mut rng = SeededRng::new(self.now);
        for index in 0..self.payments.len() {
            let into = *self
                .settling
                .ledgers(self.payments[index].to)
                .last()
                .unwrap();
            let payment = &mut self.payments[index];
            if payment.done {
                continue;
            }
            // An incoming payment that takes less stops the stream there.
            // Its receiver may have all of it before the sender counts it:
            // a hop that crashed counted a Fulfill the sender never got,
            // and the receiver says so (spec/decisions/own-fulfills.md).
            // The receiver reports what its connection key delivered, over
            // every connection the client made with the same credentials:
            // it always counts against a limit, the payment's or the most
            // any payment takes.
            let expected = payment.amount.min(payment.limit.unwrap_or(u64::MAX));
            let remote = payment
                .stream
                .and_then(|id| payment.connection.stream(id))
                .map_or(0, dassie_stream::Stream::remote_received);
            // An incoming payment that has all but its tolerance is paid in
            // full, and takes no more (spec/decisions/paid-within-a-tolerance.md).
            let full = payment.limit.is_some_and(|limit| {
                expected == limit && paid_in_full(Some(limit), remote, into.asset().scale)
            });
            if sent(payment) == expected || remote >= expected || full {
                payment.done = true;
                continue;
            }
            // The connection closed, its receiver gone or out of reach: a
            // sender starts anew for the rest, a while later.
            let closed = payment.connection.is_closed() && payment.in_flight.is_empty();
            if closed {
                payment
                    .rate_retry_at
                    .get_or_insert(self.now + RATE_RETRY_MS);
            }
            // A measurement of the rate fails when no probe gets through,
            // as across a cut link. And money stops once packets kept
            // failing: the receiver got less than the rate promises, as
            // when a route through a hop that charges less is withdrawn and
            // the packets go through one that charges more, or nodes on the
            // path failed them, as when no route is left. The library
            // reports either and leaves the rest to its user: this sender,
            // which pays an amount whatever the path delivers, measures the
            // rate again a while later, and sends at what the path delivers
            // now.
            let mut taken_all = false;
            while let Some(event) = payment.connection.poll_event() {
                if matches!(
                    event,
                    dassie_stream::Event::ExchangeRateMeasurementFailed
                        | dassie_stream::Event::MoneyStopped { .. }
                ) {
                    payment.rate_retry_at = Some(self.now + RATE_RETRY_MS);
                }
                // Its receiver says it has all it asked for.
                taken_all |= event
                    == dassie_stream::Event::ConnectionClosed {
                        code: dassie_stream::ErrorCode::NO_ERROR,
                        by_remote: true,
                    };
            }
            if taken_all && payment.limit.is_some() && payment.in_flight.is_empty() {
                payment.done = true;
                continue;
            }
            if payment.rate_retry_at.is_some_and(|at| at <= self.now) {
                payment.rate_retry_at = None;
                // Its receiver takes nothing more once its incoming payment
                // expired, as one does a day after it was made unless it
                // says otherwise: a sender that tries again stops there,
                // with what got there. The trace names it, since the checks
                // then no longer ask for all of its limit.
                if lapsed(&self.nodes, self.now, payment) {
                    payment.done = true;
                    payment.expired = true;
                    *self.counts.entry("payments expired").or_default() += 1;
                    self.trace.push(format!(
                        "{} pay {} -> {} stops, its incoming payment expired, having sent {}",
                        self.now,
                        payment.from,
                        payment.to,
                        sent(payment)
                    ));
                    continue;
                }
                if closed {
                    payment.sent_before = sent(payment);
                    payment.connection = client(&payment.credentials, payment.ledger);
                    payment.stream = None;
                } else {
                    payment.connection.measure_exchange_rate();
                }
            } else if closed {
                continue;
            }
            if payment.stream.is_none() && payment.connection.exchange_rate().is_some() {
                let id = payment.connection.open_stream().unwrap();
                let left = payment.amount - payment.sent_before;
                payment.connection.set_send_max(id, left).unwrap();
                payment.stream = Some(id);
            }
            let mut sends = Vec::new();
            while let Some(transmit) = payment.connection.poll_transmit(now, &mut rng, &mut buffer)
            {
                let request = payment.next_request;
                payment.next_request += 1;
                payment.in_flight.insert(request, transmit.request_id);
                sends.push((request, buffer[..transmit.length].to_vec()));
            }
            let (from, link) = (payment.from, payment.link);
            for (request, packet) in sends {
                // A node that stops abandons its payments, which moves the
                // rest: they go on at the next turn.
                if !self.step(from, |core| core.btp_packet(link, request, &packet, false)) {
                    return;
                }
            }
        }
    }

    /// A BTP message to a payment's client.
    fn deliver_to_client(&mut self, node: usize, link: u64, message: Outbound) {
        let now = Timestamp::from_unix_ms(self.now);
        let Some(payment) = self
            .payments
            .iter_mut()
            .find(|payment| payment.from == node && payment.link == link)
        else {
            return;
        };
        let mut buffer = vec![0u8; BUFFER_LENGTH];
        match message {
            Outbound::Response { request_id, packet } => {
                if let Some(request) = payment.in_flight.remove(&request_id) {
                    payment
                        .connection
                        .handle_response(now, request, &packet, &mut buffer);
                }
            }
            Outbound::Prepare { request_id, packet } => {
                if let Ok(prepare) = Prepare::decode_exact(&packet) {
                    let mut rng = SeededRng::new(u64::from(request_id));
                    let length =
                        payment
                            .connection
                            .handle_prepare(now, &mut rng, &prepare, &mut buffer);
                    let reply = buffer[..length].to_vec();
                    self.step(node, |core| core.btp_packet(link, request_id, &reply, true));
                }
            }
            Outbound::Error { .. } | Outbound::Close => {}
        }
    }

    /// Node `index`'s core, which must be running.
    pub fn core(&self, index: usize) -> &Core {
        self.nodes[index].core.as_ref().unwrap()
    }

    /// Whether node `index` lists `peer` as its neighbor.
    pub fn advertises(&self, index: usize, peer: &NodeId) -> bool {
        self.core(index)
            .network()
            .peers
            .iter()
            .any(|view| view.id == *peer && view.advertised)
    }

    /// Node `index`'s peers.
    pub fn peers(&self, index: usize) -> BTreeSet<NodeId> {
        self.core(index)
            .network()
            .peers
            .iter()
            .map(|peer| peer.id)
            .collect()
    }

    /// Each account's net credit, by name: a run has one ledger.
    pub fn balances(&self, index: usize) -> BTreeMap<String, i128> {
        self.core(index)
            .account_list()
            .iter()
            .map(|(_, account)| {
                let path = account.path().to_string();
                let name = path.split_once(':').map_or(path.as_str(), |(_, name)| name);
                (name.to_string(), account.balance().net_credit())
            })
            .collect()
    }

    /// What node `index`'s owner deposited on the run's one ledger: the
    /// stub's balance, what the faucet gave above the reserve, each time it
    /// funded the node, or what the issuer minted, less what it clawed
    /// back, which is the owner's withdrawal.
    pub fn deposit(&self, index: usize) -> i128 {
        match self.settling.ledger() {
            LedgerId::XrplTestnetXrp | LedgerId::XrplXrp => {
                let fundings = i128::from(self.fundings.get(&index).copied().unwrap_or(0));
                (fundings * i128::from(FAUCET_DROPS) - 1_000_000).max(0) * 1_000
            }
            LedgerId::XrplRlusdUsd | LedgerId::XrplTestnetDsusdUsd => {
                let (minted, clawed) = self.issued.get(&index).copied().unwrap_or((0, 0));
                minted - clawed
            }
            ledger => Settling::deposit(ledger),
        }
    }

    /// What node `index` owes `peer` net, by its own books: its position
    /// less the credit it extends.
    pub fn owes(&self, index: usize, peer: &NodeId) -> i128 {
        let balances = self.balances(index);
        balances[&format!("assets/interledger/{peer}")] + balances[&format!("contra/trust/{peer}")]
    }

    /// Each account's net credit on `ledger`, by name.
    pub fn balances_on(&self, index: usize, ledger: LedgerId) -> BTreeMap<String, i128> {
        self.core(index)
            .account_list()
            .iter()
            .filter(|(_, account)| account.path().ledger == ledger)
            .map(|(_, account)| {
                let path = account.path().to_string();
                let name = path.split_once(':').map_or(path.as_str(), |(_, name)| name);
                (name.to_string(), account.balance().net_credit())
            })
            .collect()
    }
}

/// How a core's panic starts when a commit fails (`engine/core/mod.rs`).
const COMMIT_FAILED: &str = "persisting the ledger failed";

thread_local! {
    /// Whether this thread runs a step whose commit is made to fail.
    static FAILING: Cell<bool> = const { Cell::new(false) };
}

/// Keeps `finding`, of node `index` and `peer`, unless one of theirs is
/// kept, and among the first few only.
fn record_once(
    findings: &mut Vec<String>,
    index: usize,
    peer: dassie_ledger::Name,
    finding: String,
) {
    let (node, to) = (format!(" node {index}'s "), format!(" {peer} "));
    let again = findings
        .iter()
        .any(|kept| kept.contains(&node) && kept.contains(&to));
    if !again && findings.len() < 10 {
        findings.push(finding);
    }
}

/// Runs a step whose commit is made to fail, catching the panic that
/// stops the core. That panic is the fault injected, not a failure of the
/// run, so it is not printed; any other panic is, by the hook there was.
fn catching(step: impl FnOnce()) -> std::thread::Result<()> {
    static QUIET: Once = Once::new();
    QUIET.call_once(|| {
        let print = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let injected = FAILING.get()
                && info
                    .payload()
                    .downcast_ref::<String>()
                    .is_some_and(|message| message.starts_with(COMMIT_FAILED));
            if !injected {
                print(info);
            }
        }));
    });
    FAILING.set(true);
    let result = catch_unwind(AssertUnwindSafe(step));
    FAILING.set(false);
    result
}

/// Whether `payment`'s incoming payment expired before it received all
/// it asked for, on its receiver's clock.
fn lapsed(nodes: &[SimNode], now: u64, payment: &Payment) -> bool {
    let to = &nodes[payment.to];
    to.store
        .incoming_payment(payment.incoming)
        .unwrap()
        .is_some_and(|row| {
            row.completed_at_ms.is_none()
                && row.expires_at_ms <= now.saturating_add_signed(to.skew_ms)
        })
}

/// What a run counts `lie` as, once a server told it.
pub const fn told_as(lie: Lie) -> &'static str {
    match lie {
        Lie::Transaction => "lies told: transaction",
        Lie::AlteredObject => "lies told: altered-object",
        Lie::FalseAbsence => "lies told: false-absence",
        Lie::OtherBranch => "lies told: other-branch",
        Lie::OldAnchor => "lies told: old-anchor",
        Lie::OlderAnchor => "lies told: older-anchor",
        Lie::ForgedValidations => "lies told: forged-validations",
        Lie::ReplayedValidations => "lies told: replayed-validations",
        Lie::WithheldValidations => "lies told: withheld-validations",
        Lie::WrongHeader => "lies told: wrong-header",
        Lie::OtherChainSkipList => "lies told: other-chain-skip-list",
        Lie::HistoryNewestLeftOut => "lies told: history-newest-left-out",
        Lie::HistoryMiddleLeftOut => "lies told: history-middle-left-out",
        Lie::OldLedgerHint => "lies told: old-ledger-hint",
        Lie::Refusal => "lies told: refusal",
        Lie::MadeUpReset => "lies told: made-up-reset",
        Lie::NoProofs => "lies told: no-proofs",
    }
}

/// What an incoming payment received.
pub fn received(sim: &Sim, payment: &Payment) -> u64 {
    sim.nodes[payment.to]
        .store
        .incoming_payment(payment.incoming)
        .unwrap()
        .map_or(0, |row| row.received_amount)
}

/// What a payment's client sent, over all its connections.
pub fn sent(payment: &Payment) -> u64 {
    payment.sent_before
        + payment
            .stream
            .and_then(|id| payment.connection.stream(id))
            .map_or(0, |stream| stream.total_sent())
}

/// A STREAM client on `ledger` paying the receiver at `credentials`.
pub fn client(credentials: &(String, String, [u8; 32]), ledger: LedgerId) -> Connection {
    let (own, destination, secret) = credentials;
    let asset = ledger.asset();
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: secret,
        local_address: Some(Address::parse(own).unwrap()),
        remote_address: Some(Address::parse(destination).unwrap()),
        local_asset: Some(AssetDetails {
            code: asset.code,
            scale: asset.scale,
        }),
        policy: Policy::DEFAULT,
    });
    connection.measure_exchange_rate();
    connection
}

/// Node `index`'s account on the XRP Ledger testnet: the original's
/// wallet of its key.
pub fn xrpl_account(index: usize) -> dassie_xrpl::AccountId {
    xrpl_account_on(index, &TESTNET)
}

/// Node `index`'s first account on `network`, as the node derives it from
/// its key.
pub fn xrpl_account_on(index: usize, network: &Network) -> dassie_xrpl::AccountId {
    let entropy = key(index).xrpl_account_entropy(network.id, 0);
    dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account()
}

impl Sim {
    /// Node `index`'s account on the run's network.
    pub fn xrpl_account(&self, index: usize) -> dassie_xrpl::AccountId {
        xrpl_account_on(index, self.settling.network())
    }

    /// The external asset of the nodes' first account on the run's
    /// network, whose connection to the fake ledger the account keeps.
    fn xrpl_asset(&self) -> String {
        self.settling.network().first().to_string()
    }
}

impl Sim {
    /// An outgoing payment across currencies, of a fixed `debit` from
    /// `from`'s first ledger to a receiver on `to`'s last, at no worse than
    /// [`least_across`].
    pub fn pay_outgoing_across(&mut self, from: usize, to: usize, debit: u64) {
        let ledger = self.settling.ledgers(from)[0];
        let into = *self.settling.ledgers(to).last().unwrap();
        let mut incoming = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: into,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: None,
                    expires_at_ms: None,
                    metadata: None,
                    uplink: None,
                })
                .unwrap();
            incoming = Some(payment);
        });
        let incoming = incoming.unwrap();
        let receive = least_across(ledger, into, debit);
        let mut outgoing = None;
        self.step(from, |core| {
            let created = core
                .create_outgoing_payment(NewOutgoing {
                    ledger,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    grant: None,
                    quote_id: None,
                    receiver: format!("https://n{to}.test/incoming-payments/{}", incoming.id),
                    destination: incoming.ilp_address.clone(),
                    shared_secret: incoming.shared_secret,
                    debit_amount: debit,
                    receive_amount: ForeignAmount {
                        value: receive,
                        asset_code: into.asset().code.into(),
                        asset_scale: 9,
                    },
                    metadata: None,
                    recipient: None,
                    fixed_debit: true,
                })
                .unwrap();
            outgoing = Some(created.row.id);
        });
        self.outgoing
            .push((from, to, incoming.id, outgoing.unwrap()));
        self.trace
            .push(format!("{} across {from} -> {to} {debit}", self.now));
    }
}

#[cfg(test)]
mod tests {
    use super::Lost;

    fn recorded(causes: &[Lost]) -> Option<Lost> {
        let mut lost = None;
        for &cause in causes {
            Lost::record(&mut lost, cause);
        }
        lost
    }

    // spec/decisions/crash-drift.md: a peer that crashed with its Fulfill on
    // its way did not lose it if that copy then arrived and was refused, or
    // late; a refusal is never taken for a crash's, so a bug that refuses
    // Fulfills is never ruled out. The sender's crash lost it whatever came
    // after, and what lost it first stays.
    #[test]
    fn what_really_lost_a_fulfill_outranks_the_peers_crash() {
        use Lost::{Late, PeerCrash, Refused, SenderCrash};
        assert_eq!(recorded(&[PeerCrash, Refused]), Some(Refused));
        assert_eq!(recorded(&[PeerCrash, Late]), Some(Late));
        assert_eq!(recorded(&[PeerCrash, SenderCrash]), Some(SenderCrash));
        assert_eq!(recorded(&[PeerCrash, PeerCrash]), Some(PeerCrash));
        assert_eq!(recorded(&[SenderCrash, Refused]), Some(SenderCrash));
        assert_eq!(recorded(&[Refused, SenderCrash]), Some(Refused));
        assert_eq!(recorded(&[Late, PeerCrash]), Some(Late));
    }
}
