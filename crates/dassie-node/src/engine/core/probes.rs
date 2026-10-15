//! Rate probes (D9): before quoting a payment to a receiver on another
//! node, the payer measures what the path delivers with packets the
//! receiver cannot fulfill, as STREAM measures exchange rates. They move
//! no money, but hold the owner's funds while in flight, as the payment
//! would.

use dassie_ilp::{Address, Timestamp};
use dassie_ledger::LedgerId;
use dassie_stream::{AssetDetails, Config, Connection, Event, Policy, Rate, Role};

use super::payers::Sender;
use super::{Core, EngineError, Output};
use crate::log::target;

/// Most probes at once.
pub const MAX_PROBES: usize = 8;
/// How long a probe may take.
pub const PROBE_TIMEOUT_MS: u64 = 10_000;

/// A probe in progress.
pub(super) struct Probe {
    ledger: LedgerId,
    connection: Box<Connection>,
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
    /// probes run.
    pub fn start_probe(
        &mut self,
        id: u64,
        ledger: LedgerId,
        destination: &str,
        shared_secret: &[u8; 32],
    ) -> Result<(), EngineError> {
        if !self.spending.payers.contains_key(&ledger) {
            return Err(EngineError::Precondition("the ledger is not active".into()));
        }
        if self.spending.probes.len() >= MAX_PROBES {
            return Err(EngineError::Exhausted(
                "too many rates are being measured".into(),
            ));
        }
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
    fn transmit_probe(&mut self, id: u64, now: u64) {
        let at = Timestamp::from_unix_ms(now);
        loop {
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
            self.send_payer_packet(now, endpoint, local, transmit.length);
        }
    }

    /// The response to one of a probe's packets.
    pub(super) fn probe_response(&mut self, now: u64, id: u64, request: u64, response: &[u8]) {
        let Self { spending, work, .. } = self;
        let Some(probe) = spending.probes.get_mut(&id) else {
            return;
        };
        probe
            .connection
            .handle_response(Timestamp::from_unix_ms(now), request, response, work);
        self.transmit_probe(id, now);
        self.settle_probe(id, now);
    }

    /// Takes in what a probe's connection found, and ends it once it is
    /// done.
    fn settle_probe(&mut self, id: u64, now: u64) {
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
        tracing::debug!(target: target::PAYMENT, id, ?rate, "rate measured");
        self.outputs_peer.push(Output::ProbeDone { id, rate });
    }

    /// Runs the probes' timers.
    pub(super) fn probe_timers(&mut self, now: u64) {
        let at = Timestamp::from_unix_ms(now);
        let due: Vec<u64> = self
            .spending
            .probes
            .iter()
            .filter(|(_, probe)| {
                probe.deadline_ms <= now
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
