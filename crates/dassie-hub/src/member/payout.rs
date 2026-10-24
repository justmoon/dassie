//! A member's copies of its validators' payout bundles
//! (spec/designs/settlement-hubs.md#9-shutdown), without IO: once a hub
//! ended or stalled, each validator signs the payout set of its last final
//! block, and a member asks each for it, a page at a time, checks it and
//! keeps it. Once it holds a quorum's bundles of one set, it can submit
//! the set itself, its own payout among it, whether or not any validator
//! is still up.
//!
//! Each validator's bundle is checked on its own against the rules: its
//! signatures are its escrow key's and its key for the set's height
//! ([`PayoutBundle::verify`]). A member keeps, per validator, the bundle
//! of the highest block it signed, never a lower one, as the validator
//! itself never signs a lower one.

use alloc::vec::Vec;

use crate::client::Request;
use crate::escrow::{MAX_PAYOUT_BUNDLE_BYTES, PayoutBundle, PayoutEntry, PayoutError, assemble};
use crate::rules::Rules;
use crate::service::wire::PayoutPage;

/// What the member does after a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Ask the validator for the set from this transaction on.
    More(u32),
    /// The validator's bundle of the block at this height is complete,
    /// checked and kept.
    Kept(u64),
    /// Nothing new: a set it holds already, or a lower one.
    Nothing,
}

/// A set being read, a page at a time.
#[derive(Clone, Debug)]
struct Reading {
    height: u64,
    total: u32,
    entries: Vec<PayoutEntry>,
    bytes: usize,
}

/// The bundles a member holds, one per validator of its rules.
#[derive(Clone, Debug)]
pub struct Payouts {
    rules: Rules,
    reading: Vec<Option<Reading>>,
    kept: Vec<Option<PayoutBundle>>,
}

impl Payouts {
    /// None yet, for the validators of `rules`.
    #[must_use]
    pub fn new(rules: Rules) -> Self {
        let n = rules.validators.len();
        Self {
            rules,
            reading: alloc::vec![None; n],
            kept: alloc::vec![None; n],
        }
    }

    /// The rules.
    #[must_use]
    pub const fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The most transactions a payout set of the rules has: every account
    /// and withdrawal, a change of signers and the selector.
    fn most_transactions(&self) -> u32 {
        let limits = &self.rules.terms.limits;
        limits
            .accounts
            .saturating_add(limits.withdrawals)
            .saturating_add(2)
    }

    /// Keeps `bundle` as validator `index`'s, as read from the node's
    /// store, once it checks.
    ///
    /// # Errors
    ///
    /// It does not.
    pub fn restore(&mut self, index: usize, bundle: PayoutBundle) -> Result<(), PayoutError> {
        bundle.verify(&self.rules, index)?;
        if let Some(kept) = self.kept.get_mut(index) {
            *kept = Some(bundle);
        }
        Ok(())
    }

    /// What to ask validator `index`: the rest of the set it is reading,
    /// or the set from its start.
    #[must_use]
    pub fn request(&self, index: usize) -> Request {
        let from = self
            .reading
            .get(index)
            .and_then(Option::as_ref)
            .map_or(0, |reading| {
                u32::try_from(reading.entries.len()).unwrap_or(u32::MAX)
            });
        Request::Payout { from }
    }

    /// Takes a page of validator `index`'s payout set.
    ///
    /// # Errors
    ///
    /// The page, or the set it completes, is not one the validator signed
    /// of the hub's: what was read of it is dropped.
    pub fn take(&mut self, index: usize, page: PayoutPage) -> Result<Taken, PayoutError> {
        if index >= self.kept.len() {
            return Err(PayoutError::Signature(0));
        }
        if page.total == 0 || page.total > self.most_transactions() {
            return Err(PayoutError::Size);
        }
        let known = self.kept.get(index).and_then(Option::as_ref);
        if known.is_some_and(|kept| kept.height >= page.height) {
            return Ok(Taken::Nothing);
        }
        let Some(slot) = self.reading.get_mut(index) else {
            return Err(PayoutError::Signature(0));
        };
        let same = slot
            .as_ref()
            .is_some_and(|r| r.height == page.height && r.total == page.total);
        if !same {
            *slot = None;
            if page.first != 0 {
                return Ok(Taken::More(0));
            }
            *slot = Some(Reading {
                height: page.height,
                total: page.total,
                entries: Vec::new(),
                bytes: 0,
            });
        }
        self.append(index, page)
    }

    /// Adds `page` to the set validator `index` is reading, and keeps the
    /// set once it is whole and checks.
    fn append(&mut self, index: usize, page: PayoutPage) -> Result<Taken, PayoutError> {
        let Some(slot) = self.reading.get_mut(index) else {
            return Err(PayoutError::Signature(0));
        };
        let Some(reading) = slot.as_mut() else {
            return Ok(Taken::More(0));
        };
        let have = u32::try_from(reading.entries.len()).unwrap_or(u32::MAX);
        if page.first != have {
            return Ok(Taken::More(have));
        }
        let room = usize::try_from(reading.total.saturating_sub(have)).unwrap_or(0);
        if page.entries.is_empty() || page.entries.len() > room {
            *slot = None;
            return Err(PayoutError::Size);
        }
        for entry in page.entries {
            reading.bytes = reading
                .bytes
                .saturating_add(entry.transaction.len())
                .saturating_add(128);
            reading.entries.push(entry);
        }
        if reading.bytes > MAX_PAYOUT_BUNDLE_BYTES {
            *slot = None;
            return Err(PayoutError::Size);
        }
        let have = u32::try_from(reading.entries.len()).unwrap_or(u32::MAX);
        if have < reading.total {
            return Ok(Taken::More(have));
        }
        let Some(done) = slot.take() else {
            return Ok(Taken::More(0));
        };
        let bundle = PayoutBundle {
            height: done.height,
            entries: done.entries,
        };
        bundle.verify(&self.rules, index)?;
        let height = bundle.height;
        if let Some(kept) = self.kept.get_mut(index) {
            *kept = Some(bundle);
        }
        Ok(Taken::Kept(height))
    }

    /// The bundles kept, each with its validator's place.
    pub fn kept(&self) -> impl Iterator<Item = (usize, &PayoutBundle)> {
        self.kept
            .iter()
            .enumerate()
            .filter_map(|(index, kept)| kept.as_ref().map(|bundle| (index, bundle)))
    }

    /// The payout set ready to submit, in order, once a quorum's bundles
    /// sign the same set: the height it pays out, and its transactions.
    #[must_use]
    pub fn assembled(&self) -> Option<(u64, Vec<Vec<u8>>)> {
        let bundles: Vec<(usize, &PayoutBundle)> = self.kept().collect();
        assemble(&self.rules, &bundles)
    }
}
