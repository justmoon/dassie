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
use crate::validators::ValidatorSet;

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
            epoch: self.epoch(),
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
    /// last within bounds we hold a finalization of and never past the end
    /// of an epoch, and the certificates of its view and after, with their
    /// blocks.
    pub(super) fn on_fetch(&mut self, from: PeerId, fetch: Fetch) {
        let (mut delivery, bytes) = self.finalized_above(fetch.height.saturating_add(1));
        if fetch.epoch == self.epoch() {
            self.add_certified(&mut delivery, fetch.view, bytes);
        }
        if delivery.finalized.is_empty() && delivery.certified.is_empty() {
            return;
        }
        self.send(Recipient::Peer(from), &Message::Delivery(delivery));
    }

    /// The finalized blocks from `start` on that an asker can check with
    /// the sets it knows, and their size. A delivery stops at the last
    /// block of an epoch, which its own set finalized: the asker learns
    /// of the next set from the blocks before it, never from a set that
    /// blocks it cannot check name. Below what we keep, it is the last
    /// block of the asker's epoch, if we kept it, or else our tip, for the
    /// asker to load the state after it (Event::Behind).
    fn finalized_above(&self, start: u64) -> (Delivery, usize) {
        let mut delivery = Delivery::default();
        let oldest = self.history.front().map_or(0, |(_, f)| f.block.height);
        if start < oldest {
            let end = self
                .boundaries
                .iter()
                .find(|boundary| boundary.block.height >= start)
                .unwrap_or(&self.tip);
            let chain = end.chain();
            if let Some(finalization) = &end.finalization
                && chain.len() <= MAX_DELIVERED_BLOCKS
            {
                delivery.finalized = chain;
                delivery.finalization = Some(finalization.clone());
            }
            let bytes = delivery.encoded_len();
            return (delivery, bytes);
        }
        let mut bytes = 0usize;
        let mut proven = (0, 0);
        for (_, finalized) in self.history.iter().filter(|(_, f)| f.block.height >= start) {
            // A block with its finalization comes with the repeats that
            // finalization proves it through, if any.
            let chain = if finalized.finalization.is_some() {
                finalized.chain()
            } else {
                alloc::vec![finalized.block.clone()]
            };
            bytes = bytes.saturating_add(chain.iter().map(Encode::encoded_len).sum::<usize>());
            let count = delivery.finalized.len().saturating_add(chain.len());
            if count > MAX_DELIVERED_BLOCKS || bytes > DELIVERY_BYTES / 2 {
                break;
            }
            delivery.finalized.extend(chain);
            if let Some(certificate) = &finalized.finalization {
                proven = (delivery.finalized.len(), bytes);
                delivery.finalization = Some(certificate.clone());
            }
            if self.ends_epoch(finalized.block.height) {
                break;
            }
        }
        let (count, bytes) = proven;
        delivery.finalized.truncate(count);
        if count == 0 {
            delivery.finalization = None;
        }
        let bytes = bytes.saturating_add(
            delivery
                .finalization
                .as_ref()
                .map_or(0, Encode::encoded_len),
        );
        (delivery, bytes)
    }

    /// Whether block `height` is the last of an epoch we know of.
    fn ends_epoch(&self, height: u64) -> bool {
        let next = height.saturating_add(1);
        self.epochs
            .iter()
            .skip(1)
            .any(|epoch| epoch.from_height == next)
            || self.boundaries.iter().any(|end| end.block.height == height)
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
    pub(super) fn on_delivery(&mut self, app: &mut dyn Application, delivery: Delivery) {
        if let Some(finalization) = &delivery.finalization {
            self.take_finalized(app, &delivery.finalized, finalization);
        }
        for certified in delivery.certified {
            self.on_certificate(certified.certificate, certified.block);
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

    /// Moves the tip to `finalized`, a block above it whose state the host
    /// loaded (spec/designs/settlement-hubs.md#9-shutdown, "The state"),
    /// without the blocks between. The application learns no block below
    /// it.
    ///
    /// The contract that keeps a validator on the hub's own history:
    ///
    /// - `finalized` must be finalized by a set the engine knows for its
    ///   height ([`Event::Behind`] hands over only such blocks), and is
    ///   checked here before anything changes.
    /// - The host checks the state it loaded against the state root that
    ///   block carries, and so against that set's signatures.
    /// - `change` is the change of validators that verified state has
    ///   scheduled and not yet made, if any: the height its set takes over
    ///   at, at least the one after the block, and the set. It is taken
    ///   from nothing else. Taking over at the next height, it runs from
    ///   the next view; later, it waits as one the chain scheduled does.
    ///
    /// A validator behind a change thus catches up epoch by epoch: the
    /// last block of its epoch, certified by the set it knows, then the
    /// state after it, which names the next set.
    ///
    /// # Errors
    ///
    /// The finalization does not prove the block with a set we know, the
    /// block is not above the tip, or the change does not fit: before
    /// the block, or another one is waiting.
    pub fn jump(
        &mut self,
        now_ms: u64,
        finalized: Finalized,
        change: Option<(u64, ValidatorSet)>,
    ) -> Result<(), Invalid> {
        self.advance(now_ms);
        let Some(finalization) = finalized.finalization.as_ref() else {
            return Err(Invalid::Unproven);
        };
        let block = &finalized.block;
        let digests = self
            .linked(&finalized.chain(), finalization)
            .ok_or(Invalid::Unproven)?;
        let digest = digests.first().copied().ok_or(Invalid::Unproven)?;
        let ahead = (block.epoch, block.view) > (self.tip.block.epoch, self.tip.block.view);
        if block.height <= self.tip.block.height || !ahead {
            return Err(Invalid::Stale);
        }
        let height = block.height;
        if let Some((from_height, set)) = change {
            self.epochs
                .take_in(from_height, set, height)
                .map_err(|_| Invalid::Stale)?;
            self.batch.epochs = Some(self.epochs.to_vec());
        }
        self.tip_digest = digest;
        self.tip = finalized;
        self.batch.tip = Some(self.tip.clone());
        self.history.clear();
        self.history.push_back((digest, self.tip.clone()));
        self.vote_log.clear();
        self.prune();
        // Into a later epoch whose set we know: it starts.
        self.after_finalized();
        Ok(())
    }

    fn take_finalized(
        &mut self,
        app: &mut dyn Application,
        blocks: &[Block],
        finalization: &Certificate,
    ) {
        // Checked with the set of its epoch that we know: a set comes to
        // us only from blocks we checked, never from one we cannot check.
        let Some(digests) = self.linked(blocks, finalization) else {
            return self.refuse();
        };
        let content = content_len(blocks);
        let (Some(first), Some(end)) = (blocks.first(), blocks.get(content.saturating_sub(1)))
        else {
            return;
        };
        if end.height <= self.tip.block.height {
            return self.check_finalized(end, finalization);
        }
        // Beyond what we can connect: the host loads the state after it.
        if first.height > self.tip.block.height.saturating_add(1) {
            self.event(Event::Behind(Box::new(Finalized {
                block: end.clone(),
                repeats: blocks.get(content..).unwrap_or_default().to_vec(),
                finalization: Some(finalization.clone()),
            })));
            return;
        }
        if first.parent != self.tip_digest || first.epoch != self.epoch() {
            return;
        }
        let chain = digests.into_iter().zip(blocks.iter().cloned()).collect();
        self.apply_chain(app, chain, Some(finalization.clone()));
    }

    /// The digests of `blocks` if they form a chain that `finalization`
    /// proves: each the parent of the next, and the last finalized by the
    /// set of its epoch, which must be one we know and the one of its
    /// height.
    fn linked(&self, blocks: &[Block], finalization: &Certificate) -> Option<Vec<Digest>> {
        let digests = self.chained(blocks, finalization)?;
        let last = blocks.last()?;
        let set = self.epochs.for_epoch(last.epoch)?;
        if self.epochs.for_height(last.height) != set {
            return None;
        }
        verify_certificate(&self.hub, set, finalization).ok()?;
        Some(digests)
    }

    /// The digests of `blocks` if each is the parent of the next, all of
    /// one epoch, and `finalization` names the last, whoever signed it.
    /// Repeats may only end the chain, of the block before them, which
    /// must be the last of its epoch, as far as we know.
    fn chained(&self, blocks: &[Block], finalization: &Certificate) -> Option<Vec<Digest>> {
        let last = blocks.last()?;
        let content = content_len(blocks);
        let end = blocks.get(content.checked_sub(1)?)?;
        let digests: Vec<Digest> = blocks.iter().map(|block| block.digest(&self.hub)).collect();
        let chained = blocks.iter().zip(blocks.iter().skip(1)).zip(&digests).all(
            |((block, next), digest)| {
                next.parent == *digest
                    && next.epoch == block.epoch
                    && next.parent_view == block.view
                    && (next.height == block.height.saturating_add(1) || next.repeats(block))
            },
        );
        let only_repeats = blocks
            .iter()
            .skip(content)
            .all(|repeat| repeat.height == end.height && repeat.payload.is_empty());
        let repeats_fit = content == blocks.len()
            || self
                .epochs
                .boundary(end.epoch)
                .is_some_and(|boundary| end.height.saturating_add(1) == boundary);
        let proven = finalization.kind == VoteKind::Finalize
            && finalization.epoch == last.epoch
            && finalization.view == last.view
            && digests.last() == Some(&finalization.block);
        (chained && only_repeats && repeats_fit && proven).then_some(digests)
    }
}

/// How many of `blocks` come before the repeats that end them: the first
/// that repeats the block before it, and all after it, are repeats.
fn content_len(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .zip(blocks.iter().skip(1))
        .position(|(block, next)| next.repeats(block))
        .map_or(blocks.len(), |at| at.saturating_add(1))
}
