//! Which validator set runs which epoch and certifies which heights
//! (spec/designs/settlement-hubs.md#8-changing-the-validators,
//! spec/decisions/epoch-handover.md).
//!
//! A new set takes over at a height H that the ledger fixes. The old set
//! never certifies a block at H or above. Once block H − 1 is final, by its
//! own finalization or a repeat's, the new set runs the next epoch from its
//! view 1, and its first block builds on block H − 1. Every vote and block
//! names its epoch, so the two sets' views never mix, whichever certificate
//! each validator learned of H − 1's finality from.

use alloc::vec::Vec;

use super::persist::Epoch;
use crate::validators::ValidatorSet;

/// The most sets held: the running one, the one before it (for
/// certificates that arrive late), and the one taking over.
pub const MAX_EPOCHS: usize = 3;

/// The validator sets, in order of height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epochs {
    /// The oldest set held, which has started.
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
    /// has not started: stored state the engine wrote, not input.
    #[must_use]
    pub fn new(mut list: Vec<Epoch>) -> Self {
        assert!(
            !list.is_empty() && list.len() <= MAX_EPOCHS,
            "between one and MAX_EPOCHS validator sets"
        );
        assert!(
            list.windows(2).all(|pair| matches!(pair, [a, b]
                if a.from_height < b.from_height
                    && b.number == a.number.saturating_add(1)
                    && (a.started || !b.started))),
            "sets in order of height and number, started ones first"
        );
        let first = list.remove(0);
        assert!(first.started, "the first set runs");
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

    /// The epoch running: the last that started.
    #[must_use]
    pub fn running(&self) -> &Epoch {
        self.iter()
            .rev()
            .find(|epoch| epoch.started)
            .unwrap_or(&self.first)
    }

    /// The set of epoch `number`, if it is held.
    #[must_use]
    pub fn for_epoch(&self, number: u64) -> Option<&ValidatorSet> {
        self.iter()
            .find(|epoch| epoch.number == number)
            .map(|epoch| &epoch.set)
    }

    /// The set that certifies a block at `height`.
    #[must_use]
    pub fn for_height(&self, height: u64) -> &ValidatorSet {
        let found = self.iter().rev().find(|epoch| epoch.from_height <= height);
        &found.unwrap_or(&self.first).set
    }

    /// The first height epoch `number` must not certify: where the next set
    /// takes over, if one is known.
    #[must_use]
    pub fn boundary(&self, number: u64) -> Option<u64> {
        self.iter()
            .find(|epoch| epoch.number == number.saturating_add(1))
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
        if from_height <= finalized_height.saturating_add(1) {
            return Err(ReconfigureError::TooLate);
        }
        self.push(from_height, set)
    }

    /// Takes in a change named by the state after block `height`, which a
    /// host loaded and checked: its set takes over at `from_height`, at
    /// least the next height. A change already known is taken again
    /// without effect.
    ///
    /// # Errors
    ///
    /// Another change is waiting, or the height is not above the block.
    pub fn take_in(
        &mut self,
        from_height: u64,
        set: ValidatorSet,
        height: u64,
    ) -> Result<(), ReconfigureError> {
        let known = self
            .iter()
            .any(|epoch| epoch.from_height == from_height && epoch.set == set);
        if known {
            return Ok(());
        }
        if from_height <= height {
            return Err(ReconfigureError::TooLate);
        }
        self.push(from_height, set)
    }

    fn push(&mut self, from_height: u64, set: ValidatorSet) -> Result<(), ReconfigureError> {
        let last = self.later.last().unwrap_or(&self.first);
        if !last.started {
            return Err(ReconfigureError::Pending);
        }
        if from_height <= last.from_height {
            return Err(ReconfigureError::TooLate);
        }
        let number = last.number.saturating_add(1);
        if self.later.len().saturating_add(1) >= MAX_EPOCHS {
            self.first = self.later.remove(0);
        }
        self.later.push(Epoch {
            number,
            from_height,
            started: false,
            set,
        });
        Ok(())
    }

    /// Block `height` is final: a set waiting for it starts. So does one
    /// whose boundary it passed, as after loading the state of a later
    /// block. Returns the number of the epoch that started, if one did.
    pub fn finalized(&mut self, height: u64) -> Option<u64> {
        let next = height.saturating_add(1);
        let epoch = self
            .later
            .iter_mut()
            .find(|epoch| !epoch.started && epoch.from_height <= next)?;
        epoch.started = true;
        Some(epoch.number)
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
            number: 0,
            from_height: 1,
            started: true,
            set: old.clone(),
        }]);
        assert_eq!(epochs.boundary(0), None);
        assert_eq!(
            epochs.reconfigure(10, new.clone(), 9),
            Err(ReconfigureError::TooLate)
        );
        epochs.reconfigure(10, new.clone(), 3).unwrap();
        assert_eq!(
            epochs.reconfigure(12, new.clone(), 3),
            Err(ReconfigureError::Pending)
        );
        assert_eq!(epochs.boundary(0), Some(10));
        assert_eq!(epochs.running().number, 0);
        assert_eq!(epochs.for_height(10), &new);
        assert_eq!(epochs.for_epoch(1), Some(&new));
        assert_eq!(epochs.finalized(8), None);
        assert_eq!(epochs.finalized(9), Some(1));
        assert_eq!(epochs.running().number, 1);
        assert_eq!(epochs.for_epoch(0), Some(&old));
        assert_eq!(epochs.boundary(1), None);
    }
}
