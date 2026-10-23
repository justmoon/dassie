//! Catch-up: asking for what we lack, answering others, and taking in
//! certified blocks. Every block comes with a certificate that proves it, or
//! with a hash link to one that does, so a delivery needs no trust in who
//! sent it.

use alloc::boxed::Box;
use alloc::vec::Vec;

use dassie_oer::Encode;

use super::super::hooks::Application;
use super::super::io::{Event, PeerId, Recipient};
use super::super::types::{Block, Certificate, Finalized, Invalid, VoteKind, verify_certificate};
use super::super::wire::{
    Certified, Delivery, Fetch, MAX_DELIVERED_BLOCKS, MAX_DELIVERED_CERTIFICATES,
    MAX_MESSAGE_BYTES, Message,
};
use super::Engine;
use crate::digest::Digest;

/// What a delivery may fill of a message, leaving room for its framing.
const DELIVERY_BYTES: usize = MAX_MESSAGE_BYTES - 1024;

impl Engine {
    /// Asks for what we lack, at most once per fetch interval.
    pub(super) fn ask(&mut self) {
        let due = self
            .fetched_ms
            .is_none_or(|at| self.now_ms >= at.saturating_add(self.config.fetch_interval_ms));
        if !due {
            return;
        }
        let Some((view, to)) = self.want.take() else {
            return;
        };
        let fetch = Fetch {
            height: self.tip.block.height,
            view,
        };
        self.send(to, &Message::Fetch(fetch));
        self.fetched_ms = Some(self.now_ms);
        self.next_ask = self.next_ask.wrapping_add(1);
        self.stats.fetches = self.stats.fetches.saturating_add(1);
    }

    /// When the next request may go, if one waits.
    pub(super) fn next_ask_ms(&self) -> Option<u64> {
        self.want.as_ref()?;
        Some(self.fetched_ms.map_or(self.now_ms, |at| {
            at.saturating_add(self.config.fetch_interval_ms)
        }))
    }

    /// Answers a request: the finalized blocks above its height, up to the
    /// last within bounds we hold a finalization of, and the certificates
    /// of its view and after, with their blocks.
    pub(super) fn on_fetch(&mut self, from: PeerId, fetch: Fetch) {
        let mut delivery = Delivery::default();
        let mut bytes = 0usize;
        let mut proven = (0, 0);
        let start = fetch.height.saturating_add(1);
        for (_, finalized) in self.history.iter().filter(|(_, f)| f.block.height >= start) {
            bytes = bytes.saturating_add(finalized.block.encoded_len());
            if delivery.finalized.len() >= MAX_DELIVERED_BLOCKS || bytes > DELIVERY_BYTES / 2 {
                break;
            }
            delivery.finalized.push(finalized.block.clone());
            if let Some(certificate) = &finalized.finalization {
                proven = (delivery.finalized.len(), bytes);
                delivery.finalization = Some(certificate.clone());
            }
        }
        let starts = delivery
            .finalized
            .first()
            .is_some_and(|first| first.height == start);
        let (count, mut bytes) = if starts { proven } else { (0, 0) };
        delivery.finalized.truncate(count);
        if count == 0 {
            delivery.finalization = None;
        }
        // Below what we keep: our tip, certified, for the asker to load
        // the state after it (Event::Behind).
        let oldest = self.history.front().map_or(0, |(_, f)| f.block.height);
        if start < oldest && self.tip.finalization.is_some() {
            delivery.finalized = alloc::vec![self.tip.block.clone()];
            delivery.finalization = self.tip.finalization.clone();
            bytes = self.tip.block.encoded_len();
        }
        bytes = bytes.saturating_add(
            delivery
                .finalization
                .as_ref()
                .map_or(0, Encode::encoded_len),
        );
        self.add_certified(&mut delivery, fetch.view, bytes);
        if delivery.finalized.is_empty() && delivery.certified.is_empty() {
            return;
        }
        self.send(Recipient::Peer(from), &Message::Delivery(delivery));
    }

    fn add_certified(&self, delivery: &mut Delivery, from_view: u64, mut bytes: usize) {
        let certificates = self.rounds.range(from_view..).flat_map(|(_, round)| {
            [
                &round.notarization,
                &round.nullification,
                &round.finalization,
            ]
            .into_iter()
            .flatten()
        });
        for certificate in certificates {
            let block = if certificate.kind == VoteKind::Nullify {
                None
            } else {
                self.blocks.get(&certificate.block).cloned()
            };
            let certified = Certified {
                certificate: certificate.clone(),
                block,
            };
            bytes = bytes.saturating_add(certified.encoded_len());
            if delivery.certified.len() >= MAX_DELIVERED_CERTIFICATES || bytes > DELIVERY_BYTES {
                break;
            }
            delivery.certified.push(certified);
        }
    }

    /// Takes in a delivery: its finalized blocks, then its certificates.
    pub(super) fn on_delivery(
        &mut self,
        app: &mut dyn Application,
        from: PeerId,
        delivery: Delivery,
    ) {
        if let Some(finalization) = &delivery.finalization {
            self.take_finalized(app, &delivery.finalized, finalization);
        }
        for certified in delivery.certified {
            self.on_certificate(certified.certificate, certified.block, from);
        }
    }

    /// Takes in consecutive finalized blocks, the last with its
    /// finalization: checked against the set of its height, and linked
    /// to the others by their digests. Members and validators coming back
    /// hand blocks in this way too.
    pub fn take_certified(
        &mut self,
        now_ms: u64,
        app: &mut dyn Application,
        blocks: &[Block],
        finalization: &Certificate,
    ) {
        self.advance(now_ms);
        self.take_finalized(app, blocks, finalization);
        self.progress(app);
    }

    /// Moves the tip to `finalized`, a certified block above it whose
    /// state the host loaded from a validator or member, without the blocks
    /// between (spec/designs/settlement-hubs.md#9-shutdown, "The state").
    /// The application learns no block below it.
    ///
    /// # Errors
    ///
    /// Its finalization does not prove it, or it is not above the tip.
    pub fn jump(&mut self, now_ms: u64, finalized: Finalized) -> Result<(), Invalid> {
        self.advance(now_ms);
        let Some(finalization) = finalized.finalization.as_ref() else {
            return Err(Invalid::Unproven);
        };
        let block = &finalized.block;
        let digests = self
            .linked(core::slice::from_ref(block), finalization)
            .ok_or(Invalid::Unproven)?;
        let digest = digests.first().copied().ok_or(Invalid::Unproven)?;
        if block.height <= self.tip.block.height || block.view <= self.tip.block.view {
            return Err(Invalid::Stale);
        }
        let (height, view) = (block.height, block.view);
        self.tip_digest = digest;
        self.tip = finalized;
        self.batch.tip = Some(self.tip.clone());
        self.history.clear();
        self.history.push_back((digest, self.tip.clone()));
        self.vote_log.clear();
        self.prune();
        if let Some((from_height, from_view)) = self.epochs.finalized(height, view) {
            self.take_over(from_height, from_view);
        }
        if self.view <= view {
            self.enter(view.saturating_add(1));
        }
        Ok(())
    }

    fn take_finalized(
        &mut self,
        app: &mut dyn Application,
        blocks: &[Block],
        finalization: &Certificate,
    ) {
        let (Some(first), Some(last)) = (blocks.first(), blocks.last()) else {
            return;
        };
        let Some(digests) = self.chained(blocks, finalization) else {
            return self.refuse();
        };
        // Beyond what we can connect, or signed by a set we do not know yet
        // (a change scheduled within the blocks delivered, closer than a
        // delivery): the host loads the state after it, and checks its
        // certificate with the validator sets that state names.
        let set = self.epochs.for_height(last.height);
        let gap = first.height > self.tip.block.height.saturating_add(1);
        let above = last.height > self.tip.block.height;
        if gap || (above && verify_certificate(&self.hub, set, finalization).is_err()) {
            self.event(Event::Behind(Box::new(Finalized {
                block: last.clone(),
                finalization: Some(finalization.clone()),
            })));
            return;
        }
        if !above {
            if verify_certificate(&self.hub, set, finalization).is_err() {
                return self.refuse();
            }
            return self.check_finalized(last, finalization);
        }
        if first.parent != self.tip_digest {
            return;
        }
        let count = blocks.len();
        for (at, (block, digest)) in blocks.iter().zip(digests).enumerate() {
            let own = (at.saturating_add(1) == count).then(|| finalization.clone());
            self.apply_finalized(app, digest, block.clone(), own);
        }
    }

    /// The digests of `blocks` if they form a chain that `finalization`
    /// proves: each the parent of the next, and the last finalized by the
    /// set of its height.
    fn linked(&self, blocks: &[Block], finalization: &Certificate) -> Option<Vec<Digest>> {
        let digests = self.chained(blocks, finalization)?;
        let set = self.epochs.for_height(blocks.last()?.height);
        verify_certificate(&self.hub, set, finalization).ok()?;
        Some(digests)
    }

    /// The digests of `blocks` if each is the parent of the next and
    /// `finalization` names the last, whoever signed it.
    fn chained(&self, blocks: &[Block], finalization: &Certificate) -> Option<Vec<Digest>> {
        let last = blocks.last()?;
        let digests: Vec<Digest> = blocks.iter().map(|block| block.digest(&self.hub)).collect();
        let chained = blocks.iter().zip(blocks.iter().skip(1)).zip(&digests).all(
            |((block, next), digest)| {
                next.parent == *digest
                    && next.height == block.height.saturating_add(1)
                    && next.parent_view == block.view
            },
        );
        let proven = finalization.kind == VoteKind::Finalize
            && finalization.view == last.view
            && digests.last() == Some(&finalization.block);
        (chained && proven).then_some(digests)
    }
}
