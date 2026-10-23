//! The view's timers: nullifying a view whose proposal or notarization is
//! late, and rebroadcasting the nullify, with the certificate that entered
//! the view, until the view ends.

use super::super::io::Recipient;
use super::super::types::VoteKind;
use super::super::wire::Message;
use super::Engine;

impl Engine {
    /// Nullifies our view if its proposal or its notarization is late.
    /// Returns whether it did.
    pub(super) fn expire(&mut self) -> bool {
        let view = self.view;
        if self.voter(view).is_none() {
            return false;
        }
        let round = self.rounds.get(&view);
        if round.is_some_and(|round| round.voted_nullify()) {
            return false;
        }
        let proposed = round.is_some_and(|round| round.proposal.is_some());
        let now = self.now_ms;
        let late = |due: Option<u64>| due.is_some_and(|due| due <= now);
        if (!proposed && late(self.timers.leader_ms)) || late(self.timers.advance_ms) {
            if self.nullify(view) {
                return true;
            }
            // No room for the view's round: the certificates that come
            // move us on, not a timer that stays due.
            self.timers.leader_ms = None;
            self.timers.advance_ms = None;
        }
        false
    }

    /// Rebroadcasts our nullify of the view, with the best certificate
    /// for entering it, if it is time; and backs off.
    pub(super) fn retry(&mut self) {
        let Some((at, interval)) = self.timers.retry else {
            return;
        };
        if at > self.now_ms {
            return;
        }
        let view = self.view;
        let nullify = self
            .rounds
            .get(&view)
            .and_then(|round| round.own(VoteKind::Nullify).copied());
        let Some(nullify) = nullify else {
            self.timers.retry = None;
            return;
        };
        self.send(Recipient::All, &Message::Vote(nullify));
        let previous = view.saturating_sub(1);
        let entered_by = self
            .rounds
            .get(&previous)
            .and_then(|round| {
                round
                    .finalization
                    .as_ref()
                    .or(round.notarization.as_ref())
                    .or(round.nullification.as_ref())
            })
            .or_else(|| {
                self.tip
                    .finalization
                    .as_ref()
                    .filter(|finalization| finalization.view == previous)
            })
            .cloned();
        // And the finalization of our tip: a validator that missed it, but
        // holds the block, finalizes it, and learns where it stands; at the
        // end of an epoch, without it, it would stay in a set that ended.
        let tip = self
            .tip
            .finalization
            .clone()
            .filter(|tip| entered_by.as_ref() != Some(tip));
        for certificate in entered_by.into_iter().chain(tip) {
            self.send(Recipient::All, &Message::Certificate(certificate));
        }
        let next = interval.saturating_mul(2).min(self.config.max_retry_ms);
        self.timers.retry = Some((self.now_ms.saturating_add(next), next));
    }

    /// The next time something is due: a timer of our view, a proposal of
    /// ours, a rebroadcast, or a request.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<u64> {
        let round = self.rounds.get(&self.view);
        let nullified = round.is_some_and(|round| round.voted_nullify());
        let proposed = round.is_some_and(|round| round.proposal.is_some());
        let voting = self.voter(self.view).is_some() && !nullified;
        let candidates = [
            self.timers.leader_ms.filter(|_| voting && !proposed),
            self.timers.advance_ms.filter(|_| voting),
            self.timers.retry.map(|(at, _)| at),
            self.timers.propose_ms.filter(|_| voting && !proposed),
            self.timers.kick_ms,
            self.next_ask_ms(),
        ];
        candidates.into_iter().flatten().min()
    }
}
