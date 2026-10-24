//! The names' random run (spec/areas/testing.md#the-simulations,
//! spec/designs/names.md#6-why-it-holds): four bootstrap nodes, which are
//! the registrars, at most one of them faulty (equivocating, withholding
//! receipts, or tipping off an adversary that registers every name unsealed
//! at it), and core and periphery nodes that register names, now and then
//! two of them the same name at once, through a stretch of chaos (a rough
//! network, crashes, restarts and cuts). Once every unsealing has arrived,
//! the chaos comes back, without new registrations, while the names are
//! granted and read and splits wait to be released, with an honest
//! registrar restarted and cut off from the others for longer than that
//! wait; then a calm, in which the splits are released.
//!
//! Checks, over the whole run: no two nodes ever take different holders for
//! a name; the adversary never holds a name; no honest registrar releases a
//! name that a registration must hold; every bound holds; and with a
//! registrar that withholds receipts, no name is unsealed or held, which is
//! the censorship the design accepts. After the calm: every registration
//! that unsealed before any other registration of its name started is held
//! by its registrant in every node's view; and with every registrar honest,
//! no split is left.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_node_core::testing::{Outcome, Run, SeededRng};

use super::{Faults, Network, Role, START, key};
use crate::names::holders::quorum;
use crate::names::{MAX_HOLDERS, MAX_REGISTRATIONS, Name, Stage};
use crate::wire::MAX_NAME_LIST;
use crate::{Config, Event, NameEvent};

/// The production settings: on the simulated clock the hours a name takes
/// to be granted, and a split to be released, cost seconds of wall time.
const CONFIG: Config = Config::DEFAULT;

/// The registrars: the bootstrap nodes, the first of the nodes. Four
/// tolerate one faulty.
const REGISTRARS: usize = 4;

/// How long the network is rough. The guarantees hold for unsealings that
/// reach every honest registrar within their windows
/// (spec/designs/names.md#3-registering): one the chaos held up is retried
/// after a backoff at most about as long as it has been failing, so the
/// chaos ends early enough in a window for that retry, and one more, to
/// arrive, however late in the chaos the registration started.
const CHAOS_MS: u64 = 10 * 60_000;
const _: () = assert!(
    2 * CHAOS_MS + 2 * (CONFIG.min_backoff_ms + CONFIG.request_timeout_ms)
        < CONFIG.names.window_ms - CONFIG.names.window_ms / 100
);

/// How long the network then stays calm, with no registration started,
/// until every unsealing has arrived or will never be sent: a registrant
/// sends none later than a window after the registration started, by its
/// own clock, which runs at the network's rate; and one sent then takes a
/// request's time at most.
const QUIET_MS: u64 = CONFIG.names.window_ms + CONFIG.request_timeout_ms;

/// How long the network is rough again after the quiet, with no new
/// registration: while the last commitments' windows close and their names
/// are granted and read, a window, and a split's wait to be released after.
const AFTERMATH_MS: u64 = CONFIG.names.window_ms + CONFIG.names.release_after_ms;

/// How long a registrar takes to have a name in its list read by every
/// node: a poll, and a hash poll's and a download's time.
const LIST_MS: u64 =
    CONFIG.list_poll_interval_ms + CONFIG.poll_timeout_ms + CONFIG.request_timeout_ms;

/// How long an honest registrar is cut off from the other registrars, from
/// the start of the aftermath, when it restarts, and up: their lists, which
/// it lost as it restarted, are missing or stale at it for longer than a
/// split's wait to be released, and count for nothing, so it releases none
/// of the names it lists (spec/areas/network.md#releasing-a-split).
const ISOLATION_MS: u64 = CONFIG.list_fresh_ms + CONFIG.names.release_after_ms + LIST_MS;
const _: () = assert!(ISOLATION_MS < AFTERMATH_MS);

/// How long the calm lasts: at the heal every commitment's window has
/// closed, so a registrar that restarts grants at once; every list is read
/// again within a poll, a split is released after its wait, and its
/// release read.
const CALM_MS: u64 = CONFIG.names.release_after_ms + 3 * LIST_MS;

/// What a seed draws.
#[derive(Clone, Copy, Debug)]
pub struct Setup {
    /// The faulty registrar and what it does, if one is.
    pub faulty: Option<(usize, Role)>,
    /// Core nodes beside the registrars that register names.
    pub ordinary: usize,
    /// Periphery nodes, which register names too.
    pub periphery: usize,
    /// The honest registrar cut off from the others in the aftermath.
    pub isolated: usize,
}

impl Setup {
    /// The core nodes: the registrars, the ordinary ones, and the adversary
    /// last if a registrar tips one off.
    #[must_use]
    pub const fn core(&self) -> usize {
        REGISTRARS + self.ordinary + if self.adversary().is_some() { 1 } else { 0 }
    }

    /// The node a registrar tips off, if one does: the last core node.
    #[must_use]
    pub const fn adversary(&self) -> Option<usize> {
        match self.faulty {
            Some((_, Role::TipOff)) => Some(REGISTRARS + self.ordinary),
            _ => None,
        }
    }

    /// Every node.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.core() + self.periphery
    }

    /// The nodes judged: every one but the faulty registrar. The adversary
    /// runs the protocol; only what it registers is wrong.
    #[must_use]
    pub fn judged(&self) -> Vec<usize> {
        let faulty = self.faulty.map(|(index, _)| index);
        (0..self.count())
            .filter(|index| Some(*index) != faulty)
            .collect()
    }

    /// The nodes that register names of their own: the honest registrars,
    /// the ordinary nodes and the periphery nodes.
    #[must_use]
    pub fn registrants(&self) -> Vec<usize> {
        let adversary = self.adversary();
        self.judged()
            .into_iter()
            .filter(|index| Some(*index) != adversary)
            .collect()
    }
}

/// The seed's generator for the run, and what it draws first.
#[must_use]
pub fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ 0x6e61_6d65_735f_7369);
    let roles = [Role::Honest, Role::Equivocate, Role::Withhold, Role::TipOff];
    let role = roles[usize::try_from(rng.below(roles.len() as u64)).unwrap()];
    let at = usize::try_from(rng.below(REGISTRARS as u64)).unwrap();
    let faulty = (!role.is_honest()).then_some((at, role));
    let ordinary = 2 + usize::try_from(rng.below(3)).unwrap();
    let periphery = 1 + usize::try_from(rng.below(2)).unwrap();
    let honest: Vec<usize> = (0..REGISTRARS)
        .filter(|index| faulty.is_none_or(|(at, _)| at != *index))
        .collect();
    let isolated = honest[usize::try_from(rng.below(honest.len() as u64)).unwrap()];
    let setup = Setup {
        faulty,
        ordinary,
        periphery,
        isolated,
    };
    (rng, setup)
}

/// What the seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    let registrars = match setup.faulty {
        Some((index, role)) => format!("registrar {index} of {REGISTRARS} {role:?}"),
        None => format!("{REGISTRARS} honest registrars"),
    };
    let adversary = setup
        .adversary()
        .map_or(String::new(), |index| format!(", adversary {index}"));
    format!(
        "{registrars}, {} core and {} periphery nodes registering{adversary}, registrar {} cut \
         off",
        setup.ordinary, setup.periphery, setup.isolated
    )
}

/// Runs the seed and checks what it did.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (_, setup) = setup(seed);
    let (network, seen, outcome) = network(seed);
    let counts = counts(&network, &setup, &seen);
    let mut run = Run::new(outcome, network.trace);
    run.counts = counts;
    run
}

/// What a run met on its way that its end no longer shows, for its counts.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    /// The judged nodes whose view was frozen while it held names, after a
    /// step of the aftermath.
    pub frozen: BTreeSet<usize>,
    /// The restarts of nodes that held names in their views, as persisted.
    pub restarts_holding: u64,
    /// The names the registrar cut off listed when it was let back.
    pub kept_isolated: u64,
}

/// Runs the seed: its chaos, its quiet, its aftermath, then its calm. With
/// it, what it met on its way, and how it ended: a safety failure found
/// before the calm ends it there.
#[must_use]
pub fn network(seed: u64) -> (Network, Seen, Outcome) {
    let (mut rng, setup) = setup(seed);
    let core = setup.core();
    let mut network = Network::with_periphery(seed, core, REGISTRARS, setup.periphery, CONFIG);
    if let Some((index, role)) = setup.faulty {
        network.roles.insert(index, role);
    }
    network.adversary = setup.adversary();
    network.faults = Faults::ROUGH;
    let registrants = setup.registrants();
    let judged = setup.judged();
    let mut seen = Seen::default();
    let mut names = 0u32;
    let mut at = START;
    while at < START + CHAOS_MS {
        at += 2_000 + rng.below(3_000);
        network.run_until(at);
        if let Err(outcome) = bounds(&network) {
            return (network, seen, outcome);
        }
        chaos(
            &mut network,
            &mut rng,
            &registrants,
            &mut names,
            &mut seen,
            None,
        );
    }
    heal(&mut network);
    let aftermath = START + CHAOS_MS + QUIET_MS;
    network.run_until(aftermath);
    if let Err(outcome) = bounds(&network) {
        return (network, seen, outcome);
    }
    // Every unsealing has arrived, or never will: the chaos comes back,
    // with no registration started, and an honest registrar restarts cut
    // off from the others. It stays up while it is, so that its waits to
    // release a split, which a restart starts again, run their course.
    network.faults = Faults::ROUGH;
    isolate(&mut network, setup.isolated, true);
    if network.is_up(setup.isolated) {
        network.crash(setup.isolated);
    }
    network.start(setup.isolated);
    let mut isolated = true;
    at = aftermath;
    while at < aftermath + AFTERMATH_MS {
        at += 2_000 + rng.below(3_000);
        network.run_until(at);
        if let Err(outcome) = bounds(&network) {
            return (network, seen, outcome);
        }
        if isolated && at >= aftermath + ISOLATION_MS {
            seen.kept_isolated = count(network.listed(setup.isolated).len());
            isolate(&mut network, setup.isolated, false);
            isolated = false;
        }
        for index in judged.iter().copied().filter(|index| network.is_up(*index)) {
            let holders = network.node(index).holders();
            if holders.is_frozen() && holders.held().next().is_some() {
                seen.frozen.insert(index);
            }
        }
        let spared = isolated.then_some(setup.isolated);
        chaos(&mut network, &mut rng, &[], &mut names, &mut seen, spared);
    }
    heal(&mut network);
    network.run_until(aftermath + AFTERMATH_MS + CALM_MS);
    let outcome = check(&network, &setup).err().unwrap_or(Outcome::Pass);
    (network, seen, outcome)
}

/// Ends the chaos: a calm network, every cut healed and every node up.
fn heal(network: &mut Network) {
    network.faults = Faults::NONE;
    network.cut.clear();
    network.cut_one_way.clear();
    for index in 0..network.nodes.len() {
        if !network.is_up(index) {
            network.start(index);
        }
    }
}

/// Cuts registrar `index` off from the other registrars, both ways, or
/// lets it back. Every other node still reaches it, and it them.
fn isolate(network: &mut Network, index: usize, cut: bool) {
    for other in (0..REGISTRARS).filter(|other| *other != index) {
        for pair in [(index, other), (other, index)] {
            if cut {
                network.cut_one_way.insert(pair);
            } else {
                network.cut_one_way.remove(&pair);
            }
        }
    }
}

/// A step of chaos: a crash of any node but `spared`, a restart, a cut made
/// or healed, or, with registrants, a name registered, by one node or by
/// two at once.
fn chaos(
    network: &mut Network,
    rng: &mut SeededRng,
    registrants: &[usize],
    names: &mut u32,
    seen: &mut Seen,
    spared: Option<usize>,
) {
    let count = network.nodes.len();
    let pick = usize::try_from(rng.below(count as u64)).unwrap();
    let registrant = |rng: &mut SeededRng| {
        registrants[usize::try_from(rng.below(registrants.len() as u64)).unwrap()]
    };
    match rng.below(12) {
        0 if network.up().count() * 3 > count * 2 && spared != Some(pick) => network.crash(pick),
        1 if !network.is_up(pick) => {
            if !network.nodes[pick].stored_holders.is_empty() {
                seen.restarts_holding += 1;
            }
            network.start(pick);
        }
        2 => {
            let other = usize::try_from(rng.below(count as u64)).unwrap();
            if other != pick {
                network.cut.insert((pick, other));
            }
        }
        3 => {
            let first = network.cut.iter().next().copied();
            if let Some(cut) = first {
                network.cut.remove(&cut);
            }
        }
        4 if !registrants.is_empty() => {
            let name = fresh(names);
            register(network, registrant(rng), name);
        }
        5 if !registrants.is_empty() && rng.below(3) == 0 => {
            let name = fresh(names);
            let (first, second) = (registrant(rng), registrant(rng));
            register(network, first, name);
            if second != first {
                register(network, second, name);
            }
        }
        _ => {}
    }
}

/// A name no node registered before.
fn fresh(names: &mut u32) -> Name {
    *names += 1;
    Name::parse(&format!("name{names}")).unwrap()
}

/// Node `index` registers `name`, as its owner asks, if it is up. Its node
/// may refuse, as it would its owner.
fn register(network: &mut Network, index: usize, name: Name) {
    if network.is_up(index) {
        network
            .trace
            .push(format!("{} {index} registers {name}", network.now));
        network.act(index, |node, now| {
            let _ = node.register_name(now, name);
        });
    }
}

/// Every bound holds at every node that is up: its registrar's log and
/// names, its list, its view and its registrations.
fn bounds(network: &Network) -> Result<(), Outcome> {
    for index in network.up() {
        let node = network.node(index);
        if let Some(registrar) = node.registrar() {
            let config = registrar.config();
            let (logged, granted) = (registrar.logged().count(), registrar.granted().count());
            if logged > config.max_commitments || logged + granted > config.max_names {
                return Err(Outcome::safety(
                    "bounds",
                    format!("node {index} keeps {logged} commitments and {granted} names"),
                ));
            }
            if network.listed(index).len() > MAX_NAME_LIST {
                return Err(Outcome::safety(
                    "bounds",
                    format!("node {index} lists more names than a list holds"),
                ));
            }
        }
        let held = node.holders().held().count();
        let registrations = node.registrant().registrations().count();
        if held > MAX_HOLDERS || registrations > MAX_REGISTRATIONS {
            return Err(Outcome::safety(
                "bounds",
                format!("node {index} holds {held} names and {registrations} registrations"),
            ));
        }
    }
    Ok(())
}

/// How the run ended: a safety failure if two nodes ever took different
/// holders for a name, the adversary held one, an honest registrar released
/// a name a registration must hold, a bound broke, or a registrar that
/// withholds receipts let a name through; a liveness failure if, once calm,
/// an honest registration is not held everywhere, or, with every registrar
/// honest, a split is left.
fn check(network: &Network, setup: &Setup) -> Result<(), Outcome> {
    let judged = setup.judged();
    bounds(network)?;
    one_holder(network, &judged).map_err(|problem| Outcome::safety("one-holder", problem))?;
    if let Some(adversary) = setup.adversary() {
        not_adversary(network, &judged, adversary)
            .map_err(|problem| Outcome::safety("no-front-running", problem))?;
    }
    kept(network, setup, &judged).map_err(|problem| Outcome::safety("kept", problem))?;
    if matches!(setup.faulty, Some((_, Role::Withhold))) {
        return withheld(network, setup);
    }
    held(network, setup, &judged).map_err(|problem| Outcome::liveness("held", problem))?;
    if setup.faulty.is_none() {
        released(network).map_err(|problem| Outcome::liveness("released", problem))?;
    }
    Ok(())
}

/// The name events, in the order they happened, with the node that
/// reported each.
fn name_events(network: &Network) -> impl Iterator<Item = (usize, &NameEvent)> {
    network
        .events
        .iter()
        .filter_map(|(index, event)| match event {
            Event::Name(event) => Some((*index, event)),
            _ => None,
        })
}

/// No two judged nodes ever took different holders for a name: at every
/// change of a view, by the views of the others at that moment, frozen or
/// not.
fn one_holder(network: &Network, judged: &[usize]) -> Result<(), String> {
    let mut views: BTreeMap<usize, BTreeMap<Name, NodeId>> = BTreeMap::new();
    for (index, event) in name_events(network) {
        if !judged.contains(&index) {
            continue;
        }
        match event {
            NameEvent::Held { name, holder } => {
                let other = views.iter().find(|(other, view)| {
                    **other != index && view.get(name).is_some_and(|held| held != holder)
                });
                if let Some((other, view)) = other {
                    return Err(format!(
                        "node {index} takes {holder} for {name}, node {other} {}",
                        view[name]
                    ));
                }
                views.entry(index).or_default().insert(*name, *holder);
            }
            NameEvent::Unheld { name, .. } => {
                views.entry(index).or_default().remove(name);
            }
            _ => {}
        }
    }
    Ok(())
}

/// The adversary, which registers only the names it was told of after they
/// were unsealed, holds none in any judged node's view, ever.
fn not_adversary(network: &Network, judged: &[usize], adversary: usize) -> Result<(), String> {
    let id = key(adversary).node_id();
    let held = name_events(network).find(|(index, event)| {
        judged.contains(index) && matches!(event, NameEvent::Held { holder, .. } if *holder == id)
    });
    match held {
        Some((index, event)) => Err(format!("node {index}: {event:?}, the adversary")),
        None => Ok(()),
    }
}

/// No honest registrar released a name that a registration must hold: every
/// honest registrar grants it within a window of the others, so at most the
/// faulty registrar's list lacks its holder, however stale or missing the
/// others' lists are where it is listed
/// (spec/areas/network.md#releasing-a-split).
fn kept(network: &Network, setup: &Setup, judged: &[usize]) -> Result<(), String> {
    let (registrations, _) = registrations(network, setup);
    let released = name_events(network).find(|(index, event)| {
        judged.contains(index)
            && matches!(event, NameEvent::Released { name, .. } if registrations.contains_key(name))
    });
    match released {
        Some((index, event)) => Err(format!("node {index}: {event:?}, which must hold")),
        None => Ok(()),
    }
}

/// With a registrar that withholds receipts, every registration was
/// abandoned before any registrar learnt its name, and no node holds one:
/// the censorship the design accepts (spec/designs/names.md#6-why-it-holds),
/// and no more.
fn withheld(network: &Network, setup: &Setup) -> Result<(), Outcome> {
    let through = name_events(network).find(|(_, event)| {
        matches!(
            event,
            NameEvent::Unsealing { .. }
                | NameEvent::Unsealed { .. }
                | NameEvent::Granted { .. }
                | NameEvent::Held { .. }
        )
    });
    if let Some((index, event)) = through {
        return Err(Outcome::safety(
            "withheld",
            format!("node {index}: {event:?}, with a registrar withholding receipts"),
        ));
    }
    for index in setup.registrants() {
        let registrant = network.node(index).registrant();
        if let Some(registration) = registrant
            .registrations()
            .find(|registration| registration.stage != Stage::Abandoned)
        {
            return Err(Outcome::liveness(
                "withheld",
                format!(
                    "node {index}'s registration of {} is not abandoned",
                    registration.name
                ),
            ));
        }
    }
    Ok(())
}

/// The registrations that must hold: those that unsealed, by an honest
/// registrant, before any other registration of the name started, each
/// with its registrant; and the names contested: registered twice before
/// either unsealed.
fn registrations(network: &Network, setup: &Setup) -> (BTreeMap<Name, usize>, BTreeSet<Name>) {
    let registrants = setup.registrants();
    let mut started: BTreeSet<Name> = BTreeSet::new();
    let mut first: BTreeMap<Name, usize> = BTreeMap::new();
    let mut contested: BTreeSet<Name> = BTreeSet::new();
    for (index, event) in name_events(network) {
        match event {
            NameEvent::Registering { name } => {
                // Another registration started before this one unsealed
                // contests it; one started after it unsealed is later at
                // every registrar.
                if !started.insert(*name) && !first.contains_key(name) {
                    contested.insert(*name);
                }
            }
            NameEvent::Unsealing { name } if registrants.contains(&index) => {
                first.entry(*name).or_insert(index);
            }
            _ => {}
        }
    }
    first.retain(|name, _| !contested.contains(name));
    (first, contested)
}

/// Every registration that must hold is held by its registrant in every
/// judged node's view, and every view is fresh.
fn held(network: &Network, setup: &Setup, judged: &[usize]) -> Result<(), String> {
    let (registrations, _) = registrations(network, setup);
    for index in judged {
        let holders = network.node(*index).holders();
        if holders.is_frozen() {
            return Err(format!("node {index}'s view is frozen"));
        }
        for (name, registrant) in &registrations {
            let held = holders.holder(name).map(|held| held.holder);
            if held != Some(key(*registrant).node_id()) {
                return Err(format!(
                    "node {index} takes {held:?} for {name}, which node {registrant} registered"
                ));
            }
        }
    }
    Ok(())
}

/// With every registrar honest, no split is left: every name a registrar
/// lists, a quorum of them list, for one holder.
fn released(network: &Network) -> Result<(), String> {
    let lists: Vec<_> = (0..REGISTRARS).map(|index| network.listed(index)).collect();
    let names: BTreeSet<Name> = lists.iter().flat_map(|list| list.keys().copied()).collect();
    for name in names {
        let holders: BTreeSet<NodeId> = lists
            .iter()
            .filter_map(|list| list.get(&name).map(|holding| holding.holder))
            .collect();
        let listing = lists.iter().filter(|list| list.contains_key(&name)).count();
        if holders.len() > 1 || listing < quorum(REGISTRARS) {
            return Err(format!(
                "{name} is listed by {listing} registrars, for {holders:?}"
            ));
        }
    }
    Ok(())
}

/// The names any node released, so far.
fn released_names(network: &Network) -> u64 {
    count(names(network, |event| match event {
        NameEvent::Released { name, .. } => Some(*name),
        _ => None,
    }))
}

/// The names `pick` finds in the name events, each once.
fn names(network: &Network, pick: fn(&NameEvent) -> Option<Name>) -> usize {
    let names: BTreeSet<Name> = name_events(network)
        .filter_map(|(_, event)| pick(event))
        .collect();
    names.len()
}

/// `n`, as a count.
fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap()
}

/// How often a run reached what it counts, for the fixed seeds to show
/// that between them they reach every case: what its registrars do, and
/// what came of its registrations, and what the aftermath met.
fn counts(network: &Network, setup: &Setup, seen: &Seen) -> BTreeMap<&'static str, u64> {
    let named = |pick: fn(&NameEvent) -> Option<Name>| -> u64 { count(names(network, pick)) };
    let (held, contested) = registrations(network, setup);
    let adversary = setup.adversary();
    let role = |wanted: Option<Role>| u64::from(setup.faulty.map(|(_, role)| role) == wanted);
    let released = released_names(network);
    BTreeMap::from([
        ("runs with honest registrars", role(None)),
        ("equivocating registrars", role(Some(Role::Equivocate))),
        (
            "registrars withholding receipts",
            role(Some(Role::Withhold)),
        ),
        ("registrars tipping off", role(Some(Role::TipOff))),
        (
            "names granted",
            named(|event| match event {
                NameEvent::Granted { name, .. } => Some(*name),
                _ => None,
            }),
        ),
        ("registrations that must hold", count(held.len())),
        ("names contested", count(contested.len())),
        ("names released", released),
        (
            "names released with honest registrars",
            if setup.faulty.is_none() { released } else { 0 },
        ),
        ("names kept by a registrar cut off", seen.kept_isolated),
        ("views frozen while holding names", count(seen.frozen.len())),
        ("restarts holding names", seen.restarts_holding),
        ("names tipped off", count(network.tipped.len())),
        (
            "names front-run",
            count(
                name_events(network)
                    .filter(|(index, event)| {
                        Some(*index) == adversary && matches!(event, NameEvent::Registering { .. })
                    })
                    .count(),
            ),
        ),
        (
            "registrations abandoned",
            count(
                name_events(network)
                    .filter(|(_, event)| matches!(event, NameEvent::Abandoned { .. }))
                    .count(),
            ),
        ),
        (
            "crashes",
            count(
                network
                    .trace
                    .iter()
                    .filter(|line| line.split(' ').nth(1) == Some("crash"))
                    .count(),
            ),
        ),
    ])
}
