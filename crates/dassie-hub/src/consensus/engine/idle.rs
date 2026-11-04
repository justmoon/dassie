//! Blocks only when wanted (spec/decisions/blocks-when-wanted.md): a view's
//! leader and advance timers run only once a block is wanted, by the
//! application or by the engine itself, or once the leader's proposal came;
//! and the leader proposes only then. What an honest validator signs is
//! untouched: only when it nullifies, and when it proposes.
//!
//! A block is wanted when the application says so ([`Application::wanted`]),
//! once the heartbeat passed since the block the next one builds on, and
//! when the chain waits to be final: at once if the view that notarized its
//! head holds more nullify votes than let a quorum finalize it, so the head
//! can be final only through a child, and otherwise once the longest
//! rebroadcast interval passed. Never sooner: a block is final three
//! message delays after its stamp only where messages are as fast as the
//! region promises, and a shorter wait would chain empty blocks wherever
//! they are slower.

use super::super::config::MAX_VIEW_TIMEOUT_MS;
use super::super::hooks::Application;
use super::super::types::VoteKind;
use super::{Engine, MAX_ROUNDS};
use crate::digest::Digest;

/// The block the next one would build on.
#[derive(Clone, Copy, Debug)]
pub(super) struct Head {
    /// The view that certified it: the tip's, or a notarized one above.
    pub(super) view: u64,
    /// The block: a repeat stands for the block it repeats.
    pub(super) digest: Digest,
    /// The stamp of the block certified in `view`: a repeat's own.
    pub(super) stamp: u64,
}

impl Engine {
    /// The block our view would build on, as the leader would choose it
    /// (`select_parent`), without asking for anything: `None` while a block
    /// or certificate it needs is missing.
    pub(super) fn head(&self) -> Option<Head> {
        let mut at = self.view.saturating_sub(1);
        for _ in 0..=MAX_ROUNDS {
            if at <= self.tip_view() {
                return Some(Head {
                    view: self.tip_view(),
                    digest: self.tip_digest,
                    stamp: self.tip.block.timestamp_ms,
                });
            }
            let round = self.rounds.get(&at)?;
            let certified = round
                .notarization
                .as_ref()
                .or(round.finalization.as_ref())
                .map(|certificate| certificate.block);
            if let Some(digest) = certified {
                let (content, _) = self.content_of(&digest)?;
                let stamp = self.known_block(&digest)?.timestamp_ms;
                return Some(Head {
                    view: at,
                    digest: content,
                    stamp,
                });
            }
            round.nullification.as_ref()?;
            at = at.saturating_sub(1);
        }
        None
    }

    /// When a block on our view's head is wanted next, by our clock, and
    /// so when the view's timers run from: as Simplex has it, from when we
    /// entered the view, when the application always wants one or the head
    /// is missing; otherwise from now once what the application wants, the
    /// heartbeat or the chain's lag is due, never from a time before we
    /// learnt of it, which would cut the leader's time short.
    fn wanted_at(&mut self, app: &mut dyn Application) -> u64 {
        let now = self.now_ms;
        let entered = self.timers.entered_ms;
        let Some(head) = self.head() else {
            return entered;
        };
        let asked = app.wanted(&head.digest, head.stamp, now);
        if asked == 0 {
            return entered;
        }
        if asked <= now {
            self.timers.wanted_on = Some(head.stamp);
        }
        let heartbeat = head.stamp.saturating_add(self.config.heartbeat_ms);
        let at = asked.min(heartbeat).min(self.lag_at(&head));
        at.max(now)
    }

    /// When the chain that waits to be final wants a block: never if the
    /// head is final; now if the view that notarized it cannot finalize it
    /// (nullified, or with more nullify votes than a quorum allows); else
    /// the longest rebroadcast interval after its stamp, by which its
    /// finalization would have come.
    fn lag_at(&self, head: &Head) -> u64 {
        if head.view <= self.tip_view() {
            return u64::MAX;
        }
        let set = self.set();
        let stuck = self.rounds.get(&head.view).is_some_and(|round| {
            round.nullification.is_some()
                || round.count(VoteKind::Nullify) > set.len().saturating_sub(set.quorum())
        });
        if stuck {
            self.now_ms
        } else {
            head.stamp.saturating_add(self.config.max_retry_ms)
        }
    }

    /// Arms our view's leader and advance timers once a block is wanted or
    /// the leader's proposal came; until then notes when to ask again.
    pub(super) fn arm(&mut self, app: &mut dyn Application) {
        let view = self.view;
        if self.timers.armed || self.voter(view).is_none() {
            return;
        }
        let proposed = self
            .rounds
            .get(&view)
            .is_some_and(|round| round.proposal.is_some());
        let from = if proposed {
            self.now_ms
        } else {
            let at = self.wanted_at(app);
            self.timers.wanted_ms = Some(at);
            if at > self.now_ms {
                return;
            }
            at
        };
        self.arm_from(view, from);
    }

    /// Arms `view`'s timers from `now`: at once for a leader gone a while,
    /// otherwise after the timeouts, longer after views that failed.
    pub(super) fn arm_from(&mut self, view: u64, now: u64) {
        let (leader_ms, advance_ms) = if self.skips(view) {
            (now, now)
        } else {
            let factor = self.backoff_at(view);
            let wait = |timeout: u64| {
                timeout
                    .saturating_mul(factor)
                    .min(MAX_VIEW_TIMEOUT_MS.max(timeout))
            };
            (
                now.saturating_add(wait(self.config.leader_timeout_ms)),
                now.saturating_add(wait(self.config.advance_timeout_ms)),
            )
        };
        self.timers.leader_ms = Some(leader_ms);
        self.timers.advance_ms = Some(advance_ms);
        self.timers.armed = true;
        self.timers.wanted_ms = None;
    }

    /// The application has something new that may want a block (an
    /// operation, a wake): a view not armed yet asks it again at once,
    /// rather than at the next time it noted.
    pub fn work_arrived(&mut self, now_ms: u64, app: &mut dyn Application) {
        if self.timers.armed || self.voter(self.view).is_none() {
            return;
        }
        self.advance(now_ms);
        self.progress(app);
    }

    /// While our view's timers run because the application wanted a block
    /// on its head, and the leader's proposal has not come: the head's
    /// stamp. The host tells the view's leader, which may lack what we
    /// have: work some validators were sent alone, or a wake carried over
    /// from an earlier view.
    #[must_use]
    pub fn wanted_by_application(&self) -> Option<u64> {
        let proposed = self
            .rounds
            .get(&self.view)
            .is_some_and(|round| round.proposal.is_some());
        if !self.timers.armed || proposed {
            return None;
        }
        self.timers.wanted_on
    }

    /// Whether our view's timers run: a block is wanted in it, or we do
    /// not vote in it.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.timers.armed || self.voter(self.view).is_none()
    }
}
