//! Which validator set runs which views and certifies which heights
//! (spec/designs/settlement-hubs.md#8-changing-the-validators).
//!
//! A new set takes over at a height H that the ledger fixes. The old set
//! never certifies a block at H or above. Once block H − 1 is final at view
//! v, the new set runs every view from v + 1, and its first block builds on
//! block H − 1. Views of the two sets never overlap, so a vote's view says
//! which set it belongs to; a block's certificate is checked against the set
//! of its height.

use alloc::vec::Vec;

use super::persist::Epoch;
use crate::validators::ValidatorSet;

/// The most sets held: the running one, the one before it (for
/// certificates that arrive late), and the one taking over.
pub const MAX_EPOCHS: usize = 3;

/// The validator sets, in order of height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epochs {
    /// The oldest set held, which runs from its first view.
    first: Epoch,
    /// Those after it.
    later: Vec<Epoch>,
}

/// Why a new set cannot take over where asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconfigureError {
    /// A change is already waiting to take effect.
    Pending,
    /// The height is final already, or the block before it is.
    TooLate,
}

impl Epochs {
    /// The sets of `list`.
    ///
    /// # Panics
    ///
    /// If the list is empty, out of order, or too long, or its first set
    /// has no first view: stored state the engine wrote, not input.
    #[must_use]
    pub fn new(mut list: Vec<Epoch>) -> Self {
        assert!(
            !list.is_empty() && list.len() <= MAX_EPOCHS,
            "between one and MAX_EPOCHS validator sets"
        );
        assert!(
            list.windows(2)
                .all(|pair| matches!(pair, [a, b] if a.from_height < b.from_height)),
            "sets in order of height"
        );
        let first = list.remove(0);
        assert!(first.from_view.is_some(), "the first set runs");
        Self { first, later: list }
    }

    /// The sets, in order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Epoch> {
        core::iter::once(&self.first).chain(&self.later)
    }

    /// The sets, in order, to persist.
    #[must_use]
    pub fn to_vec(&self) -> Vec<Epoch> {
        self.iter().cloned().collect()
    }

    /// The set that runs `view`: the last whose first view is known and at
    /// or below it.
    #[must_use]
    pub fn for_view(&self, view: u64) -> &ValidatorSet {
        let found = self
            .iter()
            .rev()
            .find(|epoch| epoch.from_view.is_some_and(|from| from <= view));
        &found.unwrap_or(&self.first).set
    }

    /// The set that certifies a block at `height`.
    #[must_use]
    pub fn for_height(&self, height: u64) -> &ValidatorSet {
        let found = self.iter().rev().find(|epoch| epoch.from_height <= height);
        &found.unwrap_or(&self.first).set
    }

    /// The first height the set running `view` must not certify: where the
    /// next set takes over, if one is waiting or took over later.
    #[must_use]
    pub fn boundary(&self, view: u64) -> Option<u64> {
        self.iter()
            .find(|epoch| epoch.from_view.is_none_or(|from| from > view))
            .map(|next| next.from_height)
    }

    /// Takes in a new set from `from_height`, with `finalized_height` final.
    ///
    /// # Errors
    ///
    /// Another change is waiting, or the height is too close.
    pub fn reconfigure(
        &mut self,
        from_height: u64,
        set: ValidatorSet,
        finalized_height: u64,
    ) -> Result<(), ReconfigureError> {
        if self.iter().any(|epoch| epoch.from_view.is_none()) {
            return Err(ReconfigureError::Pending);
        }
        if from_height <= finalized_height.saturating_add(1) {
            return Err(ReconfigureError::TooLate);
        }
        if self.later.len().saturating_add(1) >= MAX_EPOCHS {
            self.first = self.later.remove(0);
        }
        self.later.push(Epoch {
            from_height,
            from_view: None,
            set,
        });
        Ok(())
    }

    /// Block `height` is final at `view`: a set waiting for it takes over
    /// from the next view. So does one whose boundary it passed, as after
    /// loading the state of a later block: the views between are below the
    /// tip, and never read again. Returns the first height and view of the
    /// set that took over, if one did.
    pub fn finalized(&mut self, height: u64, view: u64) -> Option<(u64, u64)> {
        let next = height.saturating_add(1);
        let epoch = self
            .later
            .iter_mut()
            .find(|epoch| epoch.from_view.is_none() && epoch.from_height <= next)?;
        let from_view = view.saturating_add(1);
        epoch.from_view = Some(from_view);
        Some((epoch.from_height, from_view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use dassie_identity::NodeKey;

    fn set(seed: u8) -> ValidatorSet {
        ValidatorSet::new(
            (0..4u8)
                .map(|i| NodeKey::from_bytes(&[seed.wrapping_add(i); 32]).public_key())
                .collect(),
        )
        .unwrap()
    }

    #[test]
    fn a_new_set_takes_over_after_the_block_before_it() {
        let (old, new) = (set(1), set(50));
        let mut epochs = Epochs::new(vec![Epoch {
            from_height: 1,
            from_view: Some(1),
            set: old.clone(),
        }]);
        assert_eq!(epochs.boundary(5), None);
        assert_eq!(
            epochs.reconfigure(10, new.clone(), 9),
            Err(ReconfigureError::TooLate)
        );
        epochs.reconfigure(10, new.clone(), 3).unwrap();
        assert_eq!(
            epochs.reconfigure(12, new.clone(), 3),
            Err(ReconfigureError::Pending)
        );
        assert_eq!(epochs.boundary(40), Some(10));
        assert_eq!(epochs.for_view(40), &old);
        assert_eq!(epochs.for_height(10), &new);
        assert_eq!(epochs.finalized(8, 30), None);
        assert_eq!(epochs.finalized(9, 33), Some((10, 34)));
        assert_eq!(epochs.for_view(33), &old);
        assert_eq!(epochs.for_view(34), &new);
        assert_eq!(epochs.boundary(34), None);
        assert_eq!(epochs.boundary(33), Some(10));
    }
}
