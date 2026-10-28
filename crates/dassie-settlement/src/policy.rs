//! When to settle with a peer, and how much (spec/decisions/own-fulfills.md,
//! spec/decisions/thresholds-in-packets.md,
//! spec/areas/settlement.md#positions-and-thresholds).
//!
//! A node knows only the credit it extends to a peer, never the credit the
//! peer extends to it. So the debtor cannot tell when it should settle:
//! the creditor can, from its own books, and asks. The creditor asks once
//! its room for the peer, what the peer may still owe it, has fallen to
//! its trigger.
//!
//! A request says that the debtor stands near the asker's trigger. The
//! debtor settles half the way from where it stands to its own trigger,
//! which lands near the middle of the two, and never so far that it would
//! have to ask in turn, however far the two books have drifted apart. It
//! settles nothing less than a minimum, so a peer that asks again and
//! again cannot make it pay a ledger fee for every few drops.
//!
//! The trigger and the minimum are counted in the largest packet, not in
//! the credit: the largest packet is how much the owner is prepared to
//! lose on a link, and the credit differs on each side. Both are simple
//! rules, to be tuned later.

use dassie_ledger::{Balance, MAX_TOTAL};

/// Our position with a peer, from its `assets/interledger` account: the
/// credit we extend plus what we owe it, net of settlements in flight.
/// It is also what the peer may still owe us. The account's limit keeps
/// it at or above zero.
#[must_use]
pub fn position(interledger: &Balance) -> u128 {
    interledger
        .credits_posted
        .saturating_sub(interledger.debits_posted)
        .saturating_sub(interledger.debits_pending)
}

/// The credit we extend to a peer, from its `contra/trust` account: what
/// was posted, not what is configured now.
#[must_use]
pub fn trust(contra: &Balance) -> u64 {
    clamp(contra.debits_posted.saturating_sub(contra.credits_posted))
}

/// Our trigger for every peer: one and a half of the largest packets.
#[must_use]
pub const fn trigger(max_packet: u64) -> u64 {
    max_packet.saturating_mul(3) / 2
}

/// The least we settle when asked: one of the largest packets.
#[must_use]
pub const fn minimum(max_packet: u64) -> u64 {
    max_packet
}

/// The least default credit (`peer_credit`) a node starts with: what keeps
/// a link settling after a crash drifted the two sides' books apart by
/// `drift`, at most what was in flight. Asked at the asker's trigger, a
/// debtor settles only if its own books leave it its trigger plus twice
/// the minimum, and a payer over a hub counts the default credit when
/// asked, whatever its hold: so with this much, a hub link survives that
/// drift even where the asker holds nothing toward it
/// (spec/areas/settlement.md#credit). It is above the least that has a
/// request to a payer with nothing held fund the minimum, by the drift and
/// a trigger.
#[must_use]
pub const fn least_credit(max_packet: u64, drift: u64) -> u64 {
    trigger(max_packet)
        .saturating_mul(2)
        .saturating_add(minimum(max_packet).saturating_mul(2))
        .saturating_add(drift)
}

/// What a payee over a settlement hub lets its payer's claims add before
/// it redeems them: two of the largest packets. Each redeem costs the
/// hub's fee, so it waits for more than a packet; what it has not taken is
/// lost if the hub fails, so it waits for little more
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
#[must_use]
pub const fn redeem_threshold(max_packet: u64) -> u64 {
    max_packet.saturating_mul(2)
}

/// The least a node keeps spendable in its settlement hub: for each of its
/// peerings over the hub, and for one while it has none, the most one
/// request to settle has it fund the hold toward the peer. A payer over a
/// hub settles as if it extended the default `credit`, with its hold
/// counted as paid in advance, so a request takes at most what
/// [`requested_amount`] gives at that credit, and never less than the
/// least settlement: kept for every peering, no settlement asked of us
/// waits for a deposit. At most half the [`hub_ceiling`], which bounds how
/// many peerings it funds fully, so that the floor and the ceiling stay far
/// enough apart that each move between the hub and the node's own account
/// carries many packets' worth (spec/designs/settlement-hubs.md#11-choosing-a-hub).
#[must_use]
pub fn hub_floor(max_packet: u64, credit: u64, peerings: u64) -> u64 {
    let peerings = usize::try_from(peerings).unwrap_or(usize::MAX);
    paced_hub_floor(max_packet, credit, core::iter::repeat_n(credit, peerings))
}

/// [`hub_floor`] for peerings each paced to count `counted` as its credit
/// ([`Pace::credit`]): per peering, what one request has the payer fund at
/// that credit with nothing held, at least the least settlement; one
/// peering at the default `credit` while there is none, and at most half
/// the [`hub_ceiling`] in all (spec/decisions/settle-by-request-rate.md).
#[must_use]
pub fn paced_hub_floor(
    max_packet: u64,
    credit: u64,
    counted: impl IntoIterator<Item = u64>,
) -> u64 {
    let (trigger, minimum) = (trigger(max_packet), minimum(max_packet));
    let each = |counted: u64| requested_amount(u128::from(counted), trigger, minimum).max(minimum);
    let mut floor = 0u64;
    let mut peerings = 0u64;
    for counted in counted {
        floor = floor.saturating_add(each(counted));
        peerings = peerings.saturating_add(1);
    }
    if peerings == 0 {
        floor = each(credit);
    }
    floor.min(hub_ceiling(max_packet) / 2)
}

/// The most a node keeps in its settlement hub, its holds toward peers and
/// theirs toward it included: a hundred of the largest packets, all that a
/// hub that fails can cost the node. Its own holds count, since each is
/// the credit it extends over the hub, paid in advance. Far enough above
/// the [`hub_floor`] that each move between the hub and the node's own
/// account carries many packets' worth
/// (spec/designs/settlement-hubs.md#11-choosing-a-hub).
#[must_use]
pub const fn hub_ceiling(max_packet: u64) -> u64 {
    max_packet.saturating_mul(100)
}

/// The credit a node extends to every peer on a scheme with unlimited trust
/// ([`SettlementScheme::unlimited_trust`](crate::SettlementScheme::unlimited_trust)),
/// whose tests are not about settlement: a billion dollars in the stub's
/// units, so no test runs out of room. At most an eighth of what an
/// account's counters hold, so that a peer's account, whose posted credits
/// are the trust and all the peer ever paid us, takes seven times as much
/// again.
pub const UNLIMITED_CREDIT: u64 = 1_000_000_000_000_000_000;
const _: () = assert!(
    UNLIMITED_CREDIT <= MAX_TOTAL / 8,
    "unlimited credit past its share"
);

/// Whether to ask a peer to settle: our room for it, our `position`, has
/// fallen to our `trigger`.
#[must_use]
pub fn ask(position: u128, trigger: u64) -> bool {
    position <= u128::from(trigger)
}

/// What to settle when a peer asks: half the way from our `position` to
/// our own `trigger` for it, and nothing if that is less than `minimum`.
/// Settling lowers our position, so we never pay ourselves into asking
/// back.
#[must_use]
pub fn requested_amount(position: u128, trigger: u64, minimum: u64) -> u64 {
    let amount = position.saturating_sub(u128::from(trigger)) / 2;
    if amount < u128::from(minimum) {
        return 0;
    }
    clamp(amount)
}

/// How many times a payer over a hub may double the credit it counts when
/// asked: 11 packets become 44 at most, past the cap of a quarter of the
/// ceiling ([`most_counted_credit`]), which is what binds at the default
/// policies.
pub const MOST_DOUBLINGS: u8 = 2;

/// A payer asked again within this many of its last funding's landing
/// times, counted from when that funding landed, doubles the credit it
/// counts: its payee spends a step in less than four times what a step
/// takes to arrive, so it waits for fundings a fifth of the time or more
/// (spec/decisions/settle-by-request-rate.md).
pub const GROW_WITHIN_LANDINGS: u64 = 4;

/// A payer not asked for this many of its last funding's landing times,
/// or for the request interval if that is shorter, halves the credit it
/// counts, once for each such span: four times the span within which it
/// doubles, so that one doubling never undoes itself.
pub const IDLE_LANDINGS: u64 = 16;

/// How a payer over a hub sizes its fundings toward one peer on one
/// method by how often it is asked (spec/decisions/settle-by-request-rate.md):
/// the credit it counts when asked, the default doubled up to
/// [`MOST_DOUBLINGS`] times, once for each request that came within
/// [`GROW_WITHIN_LANDINGS`] of the last funding's landing time after it
/// landed, and halved for each span of [`IDLE_LANDINGS`] of them without a
/// request. Pure and in integers: time is an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pace {
    /// How many times the default credit is doubled, before what being
    /// idle since `since_ms` takes off.
    doublings: u8,
    /// When the pace was last used: the last request or landing.
    since_ms: u64,
    /// When the last request came.
    asked_ms: u64,
    /// When the last funding landed, until a request uses it.
    landed_ms: Option<u64>,
    /// How long the last funding took to land, from the request it
    /// answered.
    landing_ms: u64,
}

impl Pace {
    /// A pace that starts at `now_ms`, counting the default credit.
    #[must_use]
    pub const fn new(now_ms: u64) -> Self {
        Self {
            doublings: 0,
            since_ms: now_ms,
            asked_ms: now_ms,
            landed_ms: None,
            landing_ms: 0,
        }
    }

    /// How many times the default credit is doubled at `now_ms`, once the
    /// spans without a request since the pace was last used are taken off.
    /// `every_ms` is the request interval, the longest such span.
    #[must_use]
    pub fn doublings(&self, now_ms: u64, every_ms: u64) -> u8 {
        let idle = self
            .landing_ms
            .saturating_mul(IDLE_LANDINGS)
            .clamp(1, every_ms.max(1));
        let spans = now_ms.saturating_sub(self.since_ms) / idle;
        self.doublings
            .saturating_sub(u8::try_from(spans).unwrap_or(u8::MAX))
    }

    /// A request came at `now_ms`: the credit counted doubles if it came
    /// within [`GROW_WITHIN_LANDINGS`] of the last landing time after that
    /// funding landed, each landing counting once, and is what idle spans
    /// left it otherwise.
    pub fn asked(&mut self, now_ms: u64, every_ms: u64) {
        let mut doublings = self.doublings(now_ms, every_ms);
        let window = self.landing_ms.saturating_mul(GROW_WITHIN_LANDINGS);
        if let Some(landed) = self.landed_ms.take()
            && now_ms.saturating_sub(landed) <= window
        {
            doublings = doublings.saturating_add(1).min(MOST_DOUBLINGS);
        }
        self.doublings = doublings;
        self.asked_ms = now_ms;
        self.since_ms = now_ms;
        self.check(now_ms);
    }

    /// A funding to the peer landed at `now_ms`: its landing time is
    /// counted from the last request.
    pub fn landed(&mut self, now_ms: u64, every_ms: u64) {
        self.doublings = self.doublings(now_ms, every_ms);
        self.landing_ms = now_ms.saturating_sub(self.asked_ms);
        self.landed_ms = Some(now_ms);
        self.since_ms = now_ms;
        self.check(now_ms);
    }

    /// The credit counted at `now_ms`: the default `credit`, doubled as
    /// often as the pace says, at most `most` and never less than `credit`.
    #[must_use]
    pub fn credit(&self, now_ms: u64, every_ms: u64, credit: u64, most: u64) -> u64 {
        let doublings = self.doublings(now_ms, every_ms);
        let paced = credit.saturating_mul(1u64 << doublings);
        let counted = paced.min(most).max(credit);
        assert!(counted >= credit, "never less than the default credit");
        counted
    }

    /// Whether the pace has gone back to counting the default credit at
    /// `now_ms`, with no landing a request could still double it by: it
    /// is then no different from none.
    #[must_use]
    pub fn idle(&self, now_ms: u64, every_ms: u64) -> bool {
        self.doublings(now_ms, every_ms) == 0
            && self.landed_ms.is_none_or(|landed| {
                now_ms.saturating_sub(landed) > self.landing_ms.saturating_mul(GROW_WITHIN_LANDINGS)
            })
    }

    /// The pace's invariants at `now_ms`: within its doublings, and nothing
    /// it remembers in the future.
    pub fn check(&self, now_ms: u64) {
        assert!(
            self.doublings <= MOST_DOUBLINGS,
            "doublings within their bound"
        );
        assert!(
            self.since_ms <= now_ms
                && self.asked_ms <= now_ms
                && self.landed_ms.is_none_or(|landed| landed <= now_ms),
            "a pace remembers nothing in the future"
        );
        assert!(
            self.asked_ms <= self.since_ms,
            "used at its last request at the earliest"
        );
    }
}

/// The most credit a payer over a hub counts when asked often: a quarter
/// of the [`hub_ceiling`]. A payer funds its hold toward a peer as if it
/// extended the credit it counts, its hold paid in advance, so its hold
/// toward any one peer stays below a quarter of what it keeps in its hub,
/// whatever the peer asks, and one step with nothing held is at most about
/// an eighth of it (spec/decisions/settle-by-request-rate.md).
#[must_use]
pub const fn most_counted_credit(max_packet: u64) -> u64 {
    hub_ceiling(max_packet) / 4
}

/// An amount from an account's counters, which never pass `MAX_TOTAL`.
fn clamp(value: u128) -> u64 {
    assert!(value <= u128::from(MAX_TOTAL), "an amount past MAX_TOTAL");
    u64::try_from(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: u64 = 1_100_000_000;
    /// The largest packet, 0.1 units, and what follows from it.
    const P: u64 = 100_000_000;
    const T: u64 = trigger(P);
    const M: u64 = minimum(P);

    // Unlimited trust is a hundred million times the most a node keeps in
    // its hub at the default largest packet: no test runs out of room.
    #[test]
    fn unlimited_credit_dwarfs_any_hold() {
        assert_eq!(UNLIMITED_CREDIT / hub_ceiling(P), 100_000_000);
    }

    #[test]
    fn the_thresholds_follow_the_largest_packet() {
        assert_eq!((T, M), (150_000_000, 100_000_000));
        assert_eq!(redeem_threshold(P), 200_000_000);
        // Two triggers, twice the minimum, and a crash's drift at the cap
        // of two and a half packets in flight: 7.5 packets.
        assert_eq!(least_credit(P, 250_000_000), 750_000_000);
    }

    // The least credit a node starts with keeps a hub link settling after
    // a crash's drift, with no hold of the asker's, and so a request to a
    // payer with nothing held funds at least the minimum; the default is
    // above it (spec/areas/settlement.md#credit).
    #[test]
    fn the_least_credit_survives_a_crash_and_funds_a_request() {
        let drift = 250_000_000;
        let least = least_credit(P, drift);
        assert!(C >= least);
        // Asked at the asker's trigger, with no hold of the asker's, a payer
        // stands at the credit less that trigger, and less the drift: its
        // books leave it its own trigger and twice the minimum, and it
        // funds the minimum. A little less, and it would fund nothing.
        let standing = u128::from(least - T - drift);
        assert_eq!(standing, u128::from(T + 2 * M));
        assert_eq!(requested_amount(standing, T, M), M);
        assert_eq!(requested_amount(standing - 1, T, M), 0);
    }

    // A peer is asked to settle once our room for it has fallen to our
    // trigger, whatever the credit we extend.
    #[test]
    fn a_peer_is_asked_at_the_trigger() {
        assert!(!ask(u128::from(T) + 1, T));
        assert!(ask(u128::from(T), T));
        // With the same credit both ways, a debtor asked owes about 0.95:
        // it stands at 2.05 and pays 0.95, back to the middle.
        assert_eq!(requested_amount(2_050_000_000, T, M), 950_000_000);
    }

    // Asked at a balance of 0.6 of a credit of 1.1, so at a position of
    // 0.5, a node settles half the way to its own trigger at 0.15.
    #[test]
    fn a_debtor_settles_half_the_way_to_its_own_trigger() {
        assert_eq!(requested_amount(500_000_000, T, M), 175_000_000);
    }

    // A payee whose hold is empty asks at once, and the payer, owing
    // nothing yet, funds it in advance as if it extended the default credit
    // (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    #[test]
    fn without_credit_a_peer_pays_in_advance() {
        assert!(ask(0, T));
        assert_eq!(requested_amount(u128::from(C), T, M), 475_000_000);
    }

    // Books that drifted apart: the payer stays clear of its own trigger,
    // so it does not ask back for what it paid, and the two do not settle
    // back and forth for ever.
    #[test]
    fn a_payer_never_pays_itself_into_asking() {
        let position = 865_000_000;
        let paid = requested_amount(position, T, M);
        assert_eq!(paid, 357_500_000);
        assert!(!ask(position - u128::from(paid), T));
        // At its own trigger already, it pays nothing.
        assert_eq!(requested_amount(u128::from(T), T, M), 0);
    }

    // A peer that asks again and again gets nothing less than the minimum
    // at a time, so it cannot make us pay a fee per drop.
    #[test]
    fn nothing_less_than_the_minimum_is_settled() {
        let least = u128::from(T) + 2 * u128::from(M);
        assert_eq!(requested_amount(least - 1, T, M), 0);
        assert_eq!(requested_amount(least, T, M), M);
    }

    // spec/designs/settlement-hubs.md#11-choosing-a-hub: a hub's floor is,
    // per peering over it, what one request to settle has a payer fund at
    // the default credit of eleven packets: half the way down to the
    // trigger, 4.75 packets; one peering's worth with none, never less than
    // the least settlement, and at most half the ceiling of a hundred
    // packets: ten peerings are funded fully.
    #[test]
    fn the_hub_keeps_between_its_floor_and_its_ceiling() {
        assert_eq!(hub_floor(P, C, 0), 475 * P / 100);
        assert_eq!(hub_floor(P, C, 1), 475 * P / 100);
        assert_eq!(hub_floor(P, C, 10), 475 * P / 10);
        assert_eq!(hub_floor(P, C, 11), 50 * P);
        assert_eq!(hub_floor(P, 0, 2), 2 * P, "at least the least settlement");
        assert_eq!(hub_ceiling(P), 100 * P);
    }

    /// The request interval in production, and a funding's landing time.
    const EVERY: u64 = 60_000;
    const LANDING: u64 = 800;

    /// A pace asked at `at`, whose funding lands `LANDING` later; returns
    /// when it landed.
    fn cycle(pace: &mut Pace, at: u64) -> u64 {
        pace.asked(at, EVERY);
        pace.landed(at + LANDING, EVERY);
        at + LANDING
    }

    // spec/decisions/settle-by-request-rate.md: a payer asked again within
    // four landing times of its last funding landing doubles the credit it
    // counts, at most three times, and never past half the ceiling, so at
    // the default policies the third doubling hits the cap.
    #[test]
    fn a_pace_doubles_when_asked_soon() {
        let most = most_counted_credit(P);
        let mut pace = Pace::new(0);
        let mut landed = cycle(&mut pace, 0);
        assert_eq!(pace.credit(landed, EVERY, C, most), C);
        let mut counted = std::vec::Vec::new();
        for _ in 0..6 {
            landed = cycle(&mut pace, landed + GROW_WITHIN_LANDINGS * LANDING);
            counted.push(pace.credit(landed, EVERY, C, most));
        }
        assert_eq!(counted, [2 * C, most, most, most, most, most]);
        assert_eq!(most, 25 * P);
        assert_eq!(pace.doublings(landed, EVERY), MOST_DOUBLINGS);
        // A little later than the window: no doubling, and nothing taken
        // off either.
        let mut slow = Pace::new(0);
        let landed = cycle(&mut slow, 0);
        let landed = cycle(&mut slow, landed + GROW_WITHIN_LANDINGS * LANDING + 1);
        assert_eq!(slow.credit(landed, EVERY, C, most), C);
    }

    // A landing counts once: a second request after the same landing,
    // such as one asked again after a refusal, doubles nothing.
    #[test]
    fn a_landing_doubles_once() {
        let mut pace = Pace::new(0);
        let landed = cycle(&mut pace, 0);
        pace.asked(landed + 1, EVERY);
        pace.asked(landed + 2, EVERY);
        assert_eq!(pace.doublings(landed + 2, EVERY), 1);
    }

    // A pace halves for each sixteen landing times without a request, and
    // at least once per request interval however slow its fundings land;
    // once back at the default with no landing to double by, it is idle.
    #[test]
    fn a_pace_halves_when_idle() {
        let mut pace = Pace::new(0);
        let mut landed = cycle(&mut pace, 0);
        for _ in 0..3 {
            landed = cycle(&mut pace, landed + 1);
        }
        assert_eq!(pace.doublings(landed, EVERY), 2);
        let span = IDLE_LANDINGS * LANDING;
        assert_eq!(pace.doublings(landed + span - 1, EVERY), 2);
        assert_eq!(pace.doublings(landed + span, EVERY), 1);
        assert!(!pace.idle(landed + span, EVERY));
        assert_eq!(pace.doublings(landed + 2 * span, EVERY), 0);
        assert!(pace.idle(landed + 2 * span, EVERY));
        // Asked after one span, it keeps what is left, and grows no more.
        pace.asked(landed + span, EVERY);
        assert_eq!(pace.doublings(landed + span, EVERY), 1);
        // Fundings that take longer than the interval to land halve once
        // per interval.
        let mut slow = Pace::new(0);
        slow.asked(0, EVERY);
        slow.landed(EVERY, EVERY);
        slow.asked(EVERY + 1, EVERY);
        slow.landed(2 * EVERY, EVERY);
        assert_eq!(slow.doublings(2 * EVERY, EVERY), 1);
        assert_eq!(slow.doublings(3 * EVERY, EVERY), 0);
    }

    // The floor follows the steps: per peering, what one request has it
    // fund at the credit its pace counts, at most half the ceiling.
    #[test]
    fn the_floor_follows_the_steps() {
        assert_eq!(paced_hub_floor(P, C, []), hub_floor(P, C, 0));
        assert_eq!(paced_hub_floor(P, C, [C, C]), hub_floor(P, C, 2));
        assert_eq!(paced_hub_floor(P, C, [C, 2 * C]), 1_500 * P / 100);
        let most = most_counted_credit(P);
        assert_eq!(paced_hub_floor(P, C, [most]), 1_175 * P / 100);
        assert_eq!(paced_hub_floor(P, C, [most; 4]), 47 * P);
        assert_eq!(paced_hub_floor(P, C, [most; 5]), 50 * P);
    }

    #[test]
    fn positions_and_trust_come_from_the_accounts() {
        let interledger = Balance {
            credits_posted: 10,
            debits_posted: 3,
            debits_pending: 2,
            credits_pending: 100,
        };
        assert_eq!(position(&interledger), 5);
        let contra = Balance {
            debits_posted: 9,
            credits_posted: 4,
            ..Balance::default()
        };
        assert_eq!(trust(&contra), 5);
        assert_eq!(trust(&Balance::default()), 0);
    }
}
