//! The peer node's bounds and timers (spec/areas/network.md#timers-and-limits).
//! All of them are configuration, so tests and simulations can shorten them
//! (spec/overview.md#compatibility-with-the-original).

use crate::auth::AuthConfig;

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
    /// Authentication's windows and bounds.
    pub auth: AuthConfig,
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
    /// How long a node-list hash poll may take.
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
    /// Most registrations from one /24 or /48 (D4).
    pub max_registrations_per_network: usize,
    /// Probes of registrants' and peers' URLs a node makes.
    pub probe_rate: Rate,
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
    /// further one doubles it (damping, D12).
    pub advertise_hold_min_ms: u64,
    /// The longest hold-down.
    pub advertise_hold_max_ms: u64,
    /// Advertised this long without a break, a peer's flaps are forgotten.
    pub advertise_stable_ms: u64,
    /// How long the side that terminates keeps telling the other.
    pub termination_timeout_ms: u64,
    /// How long after an owner's termination the node does not ask the
    /// peer again.
    pub owner_termination_cooldown_ms: u64,
    /// Most banned nodes.
    pub max_bans: usize,
    /// Most settlement channels: one per node ever peered with.
    pub max_channels: usize,
    /// The first wait before a settlement message is sent again.
    pub settlement_retry_min_ms: u64,
    /// The longest wait.
    pub settlement_retry_max_ms: u64,
}

impl Config {
    /// The values of the spec (spec/areas/network.md#timers-and-limits,
    /// spec/areas/settlement.md#timers-and-limits).
    pub const DEFAULT: Self = Self {
        auth: AuthConfig::DEFAULT,
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
        termination_timeout_ms: 120_000,
        owner_termination_cooldown_ms: 86_400_000,
        max_bans: 1024,
        max_channels: 512,
        settlement_retry_min_ms: 1_000,
        settlement_retry_max_ms: 3_600_000,
    };

    /// The dev and test settings (spec/areas/network.md#timers-and-limits):
    /// the same rules, faster.
    pub const DEV: Self = Self {
        list_poll_interval_ms: 1_000,
        list_fresh_ms: 10_000,
        issue_debounce_ms: 500,
        sweep_interval_ms: 1_000,
        registration_ttl_ms: 120_000,
        registration_renew_ms: 30_000,
        removal_grace_ms: 10_000,
        bridge_check_ms: 2_000,
        unreachable_grace_ms: 4_000,
        min_backoff_ms: 1_000,
        max_backoff_ms: 30_000,
        peer_dead_ms: 120_000,
        advertise_hold_min_ms: 5_000,
        advertise_hold_max_ms: 60_000,
        advertise_stable_ms: 300_000,
        termination_timeout_ms: 20_000,
        owner_termination_cooldown_ms: 300_000,
        settlement_retry_max_ms: 10_000,
        ..Self::DEFAULT
    };

    /// Most inbound peerings: the rest of the slots are for ours, so that
    /// requests alone cannot fill them (D12).
    #[must_use]
    pub const fn max_inbound(&self) -> usize {
        self.max_peers.saturating_sub(self.peer_target)
    }

    /// Most peerings we start.
    #[must_use]
    pub const fn max_outbound(&self) -> usize {
        self.peer_target + 2
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
            self.link_state_rate.burst >= 1 && self.registration_rate.burst >= 1,
            "buckets hold a token"
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
            self.advertise_stable_ms > self.peer_confirm_ms,
            "flaps are forgotten only after longer than a withdrawal takes"
        );
        assert!(
            self.max_channels >= self.max_peers && self.max_bans >= 1,
            "a channel for every peer, room for a ban"
        );
    }
}
