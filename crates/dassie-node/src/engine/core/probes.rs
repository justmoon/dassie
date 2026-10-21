//! Rate probes (spec/decisions/open-payments.md): before quoting a payment to
//! a receiver on another node, the payer measures what the path delivers with
//! packets the receiver cannot fulfill, as STREAM measures exchange rates.
//! They move no money, but hold the owner's funds while in flight, as the
//! payment would.
//!
//! Clients' quotes need no owner, so anyone can start probes. The owner's
//! Send keeps [`OWNER_PROBES`] of the slots, which no client can take, and
//! no one stranger, or network of strangers, holds more than
//! [`MAX_PROBES_PER_CLIENT`]. Apps the owner approved share the clients'
//! slots without a share of their own (spec/decisions/owners-share.md).

use dassie_ilp::{Address, Timestamp};
use dassie_ledger::LedgerId;
use dassie_node_core::rate_limit::ClientKey;
use dassie_stream::{AssetDetails, Config, Connection, Event, Policy, Rate, Role};

use super::payers::Sender;
use super::{Core, EngineError, MAX_SENDS_PER_POLL, Output};

/// Most probes at once.
pub const MAX_PROBES: usize = 8;
/// Probe slots only the owner's Send may take.
pub const OWNER_PROBES: usize = 2;
/// Most probes of one stranger, and most of strangers from one network,
/// at once.
pub const MAX_PROBES_PER_CLIENT: usize = 2;
/// How long a probe may take.
pub const PROBE_TIMEOUT_MS: u64 = 10_000;

const _: () =
    assert!(OWNER_PROBES < MAX_PROBES && MAX_PROBES_PER_CLIENT <= MAX_PROBES - OWNER_PROBES);

/// Who measures a rate: the owner's Send, an app the owner approved, or a
/// stranger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prober {
    /// The owner.
    Owner,
    /// A client the owner approved: under a grant the owner approved, or
    /// one with such a grant. It shares the clients' slots, without a
    /// share of its own or of its network's, which strangers may share.
    Approved,
    /// Any other client.
    Stranger {
        /// The client, as its grant names it.
        client: String,
        /// The network its request came from; none where it counts in the
        /// totals only.
        network: Option<ClientKey>,
    },
}

/// Whether one more probe by `prober` fits beside the probes in progress,
/// by `probers`; why not otherwise.
fn probe_room<'a>(
    probers: impl Iterator<Item = &'a Prober> + Clone,
    prober: &Prober,
) -> Result<(), &'static str> {
    if probers.clone().count() >= MAX_PROBES {
        return Err("too many rates are being measured");
    }
    if *prober == Prober::Owner {
        return Ok(());
    }
    let clients = probers.clone().filter(|other| **other != Prober::Owner);
    if clients.count() >= MAX_PROBES - OWNER_PROBES {
        return Err("too many rates are being measured");
    }
    let Prober::Stranger { client, network } = prober else {
        return Ok(());
    };
    let strangers = probers.filter_map(|other| match other {
        Prober::Owner | Prober::Approved => None,
        Prober::Stranger { client, network } => Some((client, network)),
    });
    let of_client = strangers
        .clone()
        .filter(|(other, _)| *other == client)
        .count();
    let of_network = network.map_or(0, |network| {
        strangers
            .filter(|(_, other)| **other == Some(network))
            .count()
    });
    if of_client >= MAX_PROBES_PER_CLIENT || of_network >= MAX_PROBES_PER_CLIENT {
        return Err("too many of your rates are being measured");
    }
    Ok(())
}

/// A probe in progress.
pub(super) struct Probe {
    ledger: LedgerId,
    by: Prober,
    pub connection: Box<Connection>,
    deadline_ms: u64,
    /// What it measured, once it is done: a rate, or nothing.
    done: Option<Option<Rate>>,
}

impl Core {
    /// Starts measuring the rate to the receiver at `destination` from
    /// `ledger`'s payer. [`Output::ProbeDone`] with `id` says what it
    /// measured.
    ///
    /// # Errors
    ///
    /// The ledger is not active, the address is not one, or too many
    /// probes run: in all, or of the share of `by`.
    pub fn start_probe(
        &mut self,
        id: u64,
        ledger: LedgerId,
        destination: &str,
        shared_secret: &[u8; 32],
        by: Prober,
    ) -> Result<(), EngineError> {
        if !self.spending.payers.contains_key(&ledger) {
            return Err(EngineError::Precondition("the ledger is not active".into()));
        }
        probe_room(self.spending.probes.values().map(|probe| &probe.by), &by)
            .map_err(|why| EngineError::Exhausted(why.into()))?;
        if self.spending.probes.contains_key(&id) {
            return Err(EngineError::Internal("a probe's ID is taken".into()));
        }
        let remote = Address::parse(destination)
            .map_err(|_| EngineError::Invalid("the receiver's ILP address is invalid".into()))?;
        let asset = ledger.asset();
        let mut connection = Box::new(Connection::new(&Config {
            role: Role::Client,
            shared_secret,
            local_address: None,
            remote_address: Some(remote),
            local_asset: Some(AssetDetails {
                code: asset.code,
                scale: asset.scale,
            }),
            receipt: None,
            policy: Policy::DEFAULT,
        }));
        connection.measure_exchange_rate();
        let now = self.now_ms();
        self.spending.probes.insert(
            id,
            Probe {
                ledger,
                by,
                connection,
                deadline_ms: now.saturating_add(PROBE_TIMEOUT_MS),
                done: None,
            },
        );
        self.transmit_probe(id, now);
        self.settle_probe(id, now);
        Ok(())
    }

    /// Sends what a probe's connection has to send.
    pub(super) fn transmit_probe(&mut self, id: u64, now: u64) {
        let at = Timestamp::from_unix_ms(now);
        for _ in 0..MAX_SENDS_PER_POLL {
            let Self {
                rng,
                spending,
                scratch,
                ..
            } = self;
            let Some(probe) = spending.probes.get_mut(&id) else {
                return;
            };
            if probe.done.is_some() {
                return;
            }
            let Some(endpoint) = spending
                .payers
                .get(&probe.ledger)
                .map(|payer| payer.endpoint)
            else {
                return;
            };
            let Some(transmit) = probe.connection.poll_transmit(at, rng.as_mut(), scratch) else {
                return;
            };
            let local = self.spending.next_request;
            self.spending.next_request = local.wrapping_add(1);
            self.spending
                .requests
                .insert(local, (Sender::Probe(id), transmit.request_id));
            if let Some(answer) = self.send_payer_packet(now, endpoint, local, transmit.length) {
                self.take_answer(now, local, answer);
                self.settle_probe(id, now);
            }
        }
        self.sends_left();
    }

    /// Takes in what a probe's connection found, and ends it once it is
    /// done.
    pub(super) fn settle_probe(&mut self, id: u64, now: u64) {
        let Some(probe) = self.spending.probes.get_mut(&id) else {
            return;
        };
        while let Some(event) = probe.connection.poll_event() {
            let result = match event {
                Event::ExchangeRateMeasured { rate } => Some(rate),
                Event::ExchangeRateMeasurementFailed | Event::ConnectionClosed { .. } => None,
                _ => continue,
            };
            probe.done.get_or_insert(result);
        }
        if probe.done.is_none() && now >= probe.deadline_ms {
            probe.done = Some(None);
        }
        let Some(rate) = probe.done else {
            return;
        };
        // Its packets still in flight settle without it.
        self.spending.probes.remove(&id);
        self.outputs_peer.push(Output::ProbeDone { id, rate });
    }

    /// Runs the probes' timers.
    pub(super) fn probe_timers(&mut self, now: u64) {
        let at = Timestamp::from_unix_ms(now);
        // Sends left from a step before are due now; receivers and payments
        // are polled at every step anyway.
        let left = std::mem::take(&mut self.spending.sends_left);
        let due: Vec<u64> = self
            .spending
            .probes
            .iter()
            .filter(|(_, probe)| {
                left || probe.deadline_ms <= now
                    || probe.connection.poll_timeout().is_some_and(|due| due <= at)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            if let Some(probe) = self.spending.probes.get_mut(&id)
                && probe.connection.poll_timeout().is_some_and(|due| due <= at)
            {
                probe.connection.handle_timeout(at);
            }
            self.transmit_probe(id, now);
            self.settle_probe(id, now);
        }
    }

    /// The next time a probe has something to do.
    pub(super) fn probe_deadline(&self) -> Option<u64> {
        self.spending
            .probes
            .values()
            .flat_map(|probe| {
                let timer = probe.connection.poll_timeout().map(|at| at.unix_ms);
                [Some(probe.deadline_ms), timer]
            })
            .flatten()
            .min()
    }
}

impl Core {
    /// The rate probes: within their bounds, and each connection's
    /// invariants.
    pub(super) fn check_probes(&self) {
        assert!(self.spending.probes.len() <= MAX_PROBES);
        let clients = self
            .spending
            .probes
            .values()
            .filter(|probe| probe.by != Prober::Owner)
            .count();
        assert!(
            clients <= MAX_PROBES - OWNER_PROBES,
            "clients' probes took the owner's slots"
        );
        for probe in self.spending.probes.values() {
            probe.connection.check_invariants();
        }
    }
}
