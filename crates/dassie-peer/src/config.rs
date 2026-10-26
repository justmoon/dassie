//! The peer node's bounds and timers.
//! All of them are configuration, so tests and simulations can shorten them.

use crate::names::RegistrarConfig;
use crate::names::registrar::{late_renewal_ms, least_release_ms, least_window_ms};
use crate::node::{MAX_UPLINKS, own_link_state_fits};
use crate::wire::MAX_NAME_LIST;

/// How long a registration lasts in dev.
const DEV_REGISTRATION_TTL_MS: u64 = 120_000;

/// The shortest backoff in dev.
const DEV_MIN_BACKOFF_MS: u64 = 1_000;

/// The longest backoff in dev.
const DEV_MAX_BACKOFF_MS: u64 = 30_000;

/// How often the bootstrap nodes' lists are polled in dev.
const DEV_LIST_POLL_INTERVAL_MS: u64 = 1_000;

/// How long a list stays fresh in dev.
const DEV_LIST_FRESH_MS: u64 = 10_000;

/// A commitment's window in dev: the least that lets a registrant retry.
const DEV_NAME_WINDOW_MS: u64 =
    least_window_ms(Config::DEFAULT.request_timeout_ms, DEV_MIN_BACKOFF_MS);

/// A token bucket's shape: how many at once, and how fast they refill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    /// Tokens when full.
    pub burst: u32,
    /// Time for one token to come back.
    pub refill_ms: u64,
}

/// The peer node's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Most nodes in the table, ourselves included.
    pub max_nodes: usize,
    /// Most requests of ours in flight.
    pub max_requests: usize,
    /// Most inbound requests waiting on a probe.
    pub max_deferred: usize,
    /// Most nodes registered with us.
    pub max_registrations: usize,
    /// How long a request may take, unless it says otherwise.
    pub request_timeout_ms: u64,
    /// How long a node-list hash poll may take. The original defined such
    /// a timeout but polled with the 30 s default, and its start waited
    /// for the first round; here nothing waits for a poll.
    pub poll_timeout_ms: u64,
    /// How often the bootstrap nodes' lists are polled.
    pub list_poll_interval_ms: u64,
    /// A list fetched this recently is fresh, for the membership freeze.
    pub list_fresh_ms: u64,
    /// A list not fetched for this long no longer votes.
    pub list_expiry_ms: u64,
    /// How often our own link state is issued anew at the latest.
    pub reissue_interval_ms: u64,
    /// Least time between two of our link states.
    pub issue_debounce_ms: u64,
    /// How far ahead of our clock a link state's sequence may be.
    pub max_future_ms: u64,
    /// A link state older than this is stale: refreshed, and not routed.
    pub stale_ms: u64,
    /// Longest random delay before forwarding a link state.
    pub retransmit_delay_ms: u64,
    /// Copies of a link state after which it is not forwarded.
    pub suppression_copies: u8,
    /// Least time between heartbeats.
    pub heartbeat_min_ms: u64,
    /// Random time added to it.
    pub heartbeat_jitter_ms: u64,
    /// How often a stale node is refreshed.
    pub refresh_interval_ms: u64,
    /// Percent of refreshes that ask another node than the stale one.
    pub indirect_refresh_percent: u64,
    /// How often, with nothing stale, a peer is asked for the next nodes'
    /// link states anyway: anti-entropy, so a flood lost to the network,
    /// a crash or a partition is repaired within a sweep of the table.
    pub sweep_interval_ms: u64,
    /// Longest backoff of refreshes, registrations and peering attempts.
    pub max_backoff_ms: u64,
    /// Shortest backoff of the same.
    pub min_backoff_ms: u64,
    /// How long a registration lasts.
    pub registration_ttl_ms: u64,
    /// How often we renew ours.
    pub registration_renew_ms: u64,
    /// New registrations a bootstrap node takes.
    pub registration_rate: Rate,
    /// Most registrations from one /24 or /48
    /// (spec/decisions/membership-economics.md).
    pub max_registrations_per_network: usize,
    /// Probes of registrants' and peers' URLs a node makes.
    pub probe_rate: Rate,
    /// Each /24 or /48's share of the probes: a probe of a URL given from
    /// it holds a token while in flight and gives it back if it succeeds,
    /// so failing probes from one network spend only this share.
    pub probe_rate_per_network: Rate,
    /// Most networks whose failing probes are remembered.
    pub max_probe_networks: usize,
    /// Link states one subject may issue.
    pub link_state_rate: Rate,
    /// How long a node with no reason to be in the table stays.
    pub removal_grace_ms: u64,
    /// How many peers a node looks for.
    pub peer_target: usize,
    /// Most peers.
    pub max_peers: usize,
    /// How often to look for peers.
    pub peering_check_ms: u64,
    /// How often to look for a bridge to an unreachable node.
    pub bridge_check_ms: u64,
    /// How long a node must be unreachable for a bridge to be sought.
    pub unreachable_grace_ms: u64,
    /// A heartbeat this recent confirms a peering.
    pub peer_confirm_ms: u64,
    /// Least time between two route computations.
    pub route_debounce_ms: u64,
    /// A peer not heard from for this long is terminated as unresponsive.
    pub peer_dead_ms: u64,
    /// The first hold-down of a peer that stopped being advertised; each
    /// further one doubles it (damping, spec/decisions/peering-lifecycle.md).
    pub advertise_hold_min_ms: u64,
    /// The longest hold-down.
    pub advertise_hold_max_ms: u64,
    /// Advertised this long without a break, a peer's flaps are forgotten.
    pub advertise_stable_ms: u64,
    /// A peer whose room in our books stays short of a packet for this
    /// long, without settling, is withdrawn. Two requests to settle may be
    /// lost, each costing the node's interval between requests, and the
    /// third still has a request's timeout to be answered and as long
    /// again for the settlement it asks for to be booked: two intervals
    /// and two request timeouts. One short for [`Config::peer_dead_ms`] is
    /// terminated.
    pub advertise_short_grace_ms: u64,
    /// How long the side that terminates keeps telling the other.
    pub termination_timeout_ms: u64,
    /// How long a node ending a peering waits for each of two things before
    /// it tells the peer (spec/decisions/ending-a-peering.md): its own
    /// settlements to the peer under way, and what the peer's answer to its
    /// last request says is on its way. Long enough for a settlement to reach
    /// the ledger and be announced: on the XRP Ledger, a transaction applies
    /// within its window of 20 ledgers, some 80 s, or never; with a request's
    /// timeout for its announcement, 110 s.
    pub depeer_wait_ms: u64,
    /// How long after an owner's termination the node does not ask the
    /// peer again.
    pub owner_termination_cooldown_ms: u64,
    /// Most banned nodes.
    pub max_bans: usize,
    /// Most settlement channels open with core nodes: one per core node
    /// peered with, or not long ago, since a channel closes once idle for
    /// [`Config::peer_dead_ms`] after its peering ended.
    pub max_channels: usize,
    /// Most downlinks a core node keeps, apart from its peers with other
    /// core nodes (spec/areas/network.md#downlinks): each with a
    /// settlement channel, which closes as a core node's does, so this
    /// bounds the periphery nodes peered with, or not long ago. A node
    /// without a URL, or with none of these, takes no downlinks.
    pub max_downlinks: usize,
    /// The first wait before a settlement message is sent again.
    pub settlement_retry_min_ms: u64,
    /// The longest wait.
    pub settlement_retry_max_ms: u64,
    /// How often the channels idle long enough are looked at to close:
    /// each asks the books, which scan what is pending, so a channel the
    /// books hold open is not asked at every step.
    pub channel_check_ms: u64,
    /// Whether the node takes part in names at all: as a registrar, in
    /// its view of who holds each name, and as a registrant. Off, it is no
    /// registrar, refusing every name message as any other node does, reads
    /// no name lists, holds no names and registers none. The node's driver
    /// decides; the peer protocol knows no reason
    /// (spec/areas/network.md#names).
    pub names_enabled: bool,
    /// A registrar's timers and bounds, when the node is one of its own
    /// bootstrap nodes (spec/areas/network.md#names).
    pub names: RegistrarConfig,
}

impl Config {
    /// The production values.
    pub const DEFAULT: Self = Self {
        max_nodes: 1024,
        max_requests: 8192,
        max_deferred: 32,
        max_registrations: 1024,
        request_timeout_ms: 30_000,
        poll_timeout_ms: 5_000,
        list_poll_interval_ms: 60_000,
        list_fresh_ms: 600_000,
        list_expiry_ms: 86_400_000,
        reissue_interval_ms: 3_600_000,
        issue_debounce_ms: 5_000,
        max_future_ms: 600_000,
        stale_ms: 10_800_000,
        retransmit_delay_ms: 500,
        suppression_copies: 3,
        heartbeat_min_ms: 5_000,
        heartbeat_jitter_ms: 15_000,
        refresh_interval_ms: 1_000,
        indirect_refresh_percent: 20,
        sweep_interval_ms: 10_000,
        max_backoff_ms: 3_600_000,
        min_backoff_ms: 10_000,
        registration_ttl_ms: 86_400_000,
        registration_renew_ms: 21_600_000,
        registration_rate: Rate {
            burst: 20,
            refill_ms: 60_000,
        },
        link_state_rate: Rate {
            burst: 8,
            refill_ms: 5_000,
        },
        max_registrations_per_network: 8,
        probe_rate: Rate {
            burst: 32,
            refill_ms: 1_000,
        },
        probe_rate_per_network: Rate {
            burst: 4,
            refill_ms: 60_000,
        },
        max_probe_networks: 256,
        removal_grace_ms: 3_600_000,
        peer_target: 2,
        max_peers: 8,
        peering_check_ms: 1_000,
        bridge_check_ms: 60_000,
        unreachable_grace_ms: 120_000,
        peer_confirm_ms: 60_000,
        route_debounce_ms: 250,
        peer_dead_ms: 86_400_000,
        advertise_hold_min_ms: 30_000,
        advertise_hold_max_ms: 3_600_000,
        advertise_stable_ms: 3_600_000,
        // Requests to settle every minute, each with 30 s to be answered.
        advertise_short_grace_ms: 180_000,
        termination_timeout_ms: 120_000,
        depeer_wait_ms: 120_000,
        owner_termination_cooldown_ms: 86_400_000,
        max_bans: 1024,
        max_channels: 512,
        max_downlinks: 1_000,
        settlement_retry_min_ms: 1_000,
        settlement_retry_max_ms: 3_600_000,
        channel_check_ms: 60_000,
        names_enabled: true,
        names: RegistrarConfig::DEFAULT,
    };

    /// The dev settings: the same rules, faster, for what runs on a real
    /// clock. The network simulations run [`Config::DEFAULT`].
    pub const DEV: Self = Self {
        list_poll_interval_ms: DEV_LIST_POLL_INTERVAL_MS,
        list_fresh_ms: DEV_LIST_FRESH_MS,
        issue_debounce_ms: 500,
        sweep_interval_ms: 1_000,
        registration_ttl_ms: DEV_REGISTRATION_TTL_MS,
        registration_renew_ms: 30_000,
        removal_grace_ms: 10_000,
        bridge_check_ms: 2_000,
        unreachable_grace_ms: 4_000,
        min_backoff_ms: DEV_MIN_BACKOFF_MS,
        max_backoff_ms: DEV_MAX_BACKOFF_MS,
        peer_dead_ms: 120_000,
        advertise_hold_min_ms: 5_000,
        advertise_hold_max_ms: 60_000,
        advertise_stable_ms: 300_000,
        // Requests to settle every 5 s in dev; a request's timeout is
        // still 30 s.
        advertise_short_grace_ms: 70_000,
        termination_timeout_ms: 20_000,
        depeer_wait_ms: 20_000,
        owner_termination_cooldown_ms: 300_000,
        settlement_retry_max_ms: 10_000,
        channel_check_ms: 5_000,
        // As in production, a name lasts 365 registrations, and its grace
        // and the time to renew it 30 each; the window is the least that
        // lets a registrant retry (`validate`), as dev registrars run side
        // by side, and a split is released after the least wait that lets
        // a registration, or a renewal that arrived late, reach every list.
        names: RegistrarConfig {
            window_ms: DEV_NAME_WINDOW_MS,
            ttl_ms: 365 * DEV_REGISTRATION_TTL_MS,
            grace_ms: 30 * DEV_REGISTRATION_TTL_MS,
            renew_before_ms: 30 * DEV_REGISTRATION_TTL_MS,
            release_after_ms: least_release_ms(
                DEV_NAME_WINDOW_MS,
                late_renewal_ms(Self::DEFAULT.request_timeout_ms, DEV_MAX_BACKOFF_MS),
                DEV_LIST_FRESH_MS,
                DEV_LIST_POLL_INTERVAL_MS,
                Self::DEFAULT.poll_timeout_ms + Self::DEFAULT.request_timeout_ms,
            ),
            ..RegistrarConfig::DEFAULT
        },
        ..Self::DEFAULT
    };

    /// Most inbound peerings: the rest of the slots are for ours, so that
    /// requests alone cannot fill them (spec/decisions/peering-lifecycle.md).
    #[must_use]
    pub const fn max_inbound(&self) -> usize {
        self.max_peers.saturating_sub(self.peer_target)
    }

    /// Most peerings we start.
    #[must_use]
    pub const fn max_outbound(&self) -> usize {
        self.peer_target + 2
    }

    /// Most peerings with core nodes a node holds, of every kind: a
    /// periphery node pinned to its uplinks has slots of its own, as many
    /// as it pins, up to [`MAX_UPLINKS`]; any other node at most
    /// `max_peers` (spec/areas/network.md#pinned-uplinks). What its tables,
    /// its channels and the node's connector are sized for.
    #[must_use]
    pub const fn max_core_peers(&self) -> usize {
        if self.max_peers > MAX_UPLINKS {
            self.max_peers
        } else {
            MAX_UPLINKS
        }
    }

    /// Most downlinks that never paid us, free receivers, holding places at
    /// once: half of [`Config::max_downlinks`], rounded down, so that keys
    /// made for nothing never fill the places of downlinks that paid
    /// (spec/decisions/free-receivers-half.md).
    #[must_use]
    pub const fn max_free_downlinks(&self) -> usize {
        self.max_downlinks / 2
    }

    /// Checks that the values fit together (configuration, not input).
    ///
    /// # Panics
    ///
    /// If they do not.
    pub fn validate(&self) {
        assert!(self.max_nodes >= 2, "room for us and one other");
        assert!(self.peer_target >= 1, "a node looks for peers");
        assert!(
            self.max_peers >= self.max_outbound() && self.max_inbound() >= 1,
            "room for inbound and outbound peers"
        );
        assert!(self.max_requests >= 64, "room for gossip");
        assert!(self.suppression_copies >= 1, "a copy suppresses");
        assert!(
            self.registration_renew_ms < self.registration_ttl_ms,
            "registrations are renewed before they expire"
        );
        assert!(self.min_backoff_ms <= self.max_backoff_ms, "backoff bounds");
        assert!(
            self.channel_check_ms > 0 && self.channel_check_ms < self.peer_dead_ms,
            "idle channels are looked at well within their wait"
        );
        assert!(
            self.link_state_rate.burst >= 1 && self.registration_rate.burst >= 1,
            "buckets hold a token"
        );
        assert!(
            self.probe_rate_per_network.burst >= 1
                && self.probe_rate_per_network.burst < self.probe_rate.burst
                && self.max_probe_networks >= 1,
            "a network's probes are a share of all"
        );
        assert!(self.indirect_refresh_percent <= 100, "a percentage");
        assert!(
            self.advertise_hold_min_ms <= self.advertise_hold_max_ms
                && self.settlement_retry_min_ms <= self.settlement_retry_max_ms,
            "hold-down and retry bounds"
        );
        assert!(
            self.peer_dead_ms > self.peer_confirm_ms,
            "a peer is unconfirmed before it is dead"
        );
        assert!(
            self.advertise_stable_ms > self.peer_confirm_ms
                && self.advertise_stable_ms > self.advertise_short_grace_ms,
            "flaps are forgotten only after longer than a withdrawal takes"
        );
        assert!(
            self.advertise_short_grace_ms > 0 && self.peer_dead_ms > self.advertise_short_grace_ms,
            "a peer short of room is given time to settle, and withdrawn before it is ended"
        );
        assert!(
            self.max_channels >= self.max_core_peers() && self.max_bans >= 1,
            "a channel for every peer, room for a ban"
        );
        assert!(
            self.max_downlinks == 0 || self.max_free_downlinks() >= 1,
            "a node that takes downlinks has room for a free receiver"
        );
        assert!(
            own_link_state_fits(self.max_peers),
            "our own link state always decodes"
        );
        self.validate_names();
    }

    /// Checks a registrar's values (spec/areas/network.md#names).
    fn validate_names(&self) {
        let names = &self.names;
        assert!(
            names.window_ms >= least_window_ms(self.request_timeout_ms, self.min_backoff_ms),
            "a commitment and its unsealing each get retries within the window"
        );
        assert!(
            names.window_ms < names.ttl_ms && names.renew_before_ms < names.ttl_ms,
            "a name outlives a window, and is renewed before it expires"
        );
        // A renewal is retried after the longest backoff before the name
        // expires, and one that reached a registrar late still lands in
        // its grace.
        let late_ms = late_renewal_ms(self.request_timeout_ms, self.max_backoff_ms);
        assert!(
            names.renew_before_ms >= late_ms && names.grace_ms >= late_ms,
            "renewals have time for their retries"
        );
        // A registration, and a renewal that reached a registrar late,
        // reach every registrar's list before the others count as lacking
        // them.
        let fetch_ms = self.poll_timeout_ms + self.request_timeout_ms;
        assert!(
            names.release_after_ms
                >= least_release_ms(
                    names.window_ms,
                    late_ms,
                    self.list_fresh_ms,
                    self.list_poll_interval_ms,
                    fetch_ms
                ),
            "a split is released only after a registration or a renewal reaches every list"
        );
        // Only fresh lists vote for holders, and what a list shows was
        // current up to a hash poll's and a download's time before it was
        // fetched. An honest registrar lists a new holder for a name a
        // window at least after it stopped listing the last, by its clock,
        // so no two fresh lists from it name different holders while that
        // clock never steps forward by more than the margin
        // (spec/areas/network.md#holders).
        assert!(
            names.window_ms > self.list_fresh_ms + fetch_ms,
            "a registrar's fresh lists name one holder"
        );
        assert!(
            names.max_names <= MAX_NAME_LIST && names.max_commitments < names.max_names,
            "the list served always decodes, and names have room beside the log"
        );
        assert!(
            names.commit_rate.burst >= 1
                && names.max_commitments_per_network >= 1
                && names.max_commitments_per_node >= 1
                && names.max_commitments_per_node <= names.max_names_per_node,
            "a node may commit"
        );
    }
}
