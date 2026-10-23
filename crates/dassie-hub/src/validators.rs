//! A hub's validator set: n = 3f + 1 consensus keys, the quorum q = 2f + 1,
//! and the bitmaps that say which of them signed
//! (spec/designs/settlement-hubs.md#1-concepts).

use alloc::vec::Vec;

use dassie_identity::PublicKey;

/// The most validators a hub can have. A certificate's bitmap is a `u16`,
/// and the design expects 4 or 7.
pub const MAX_VALIDATORS: usize = 16;

/// The fewest validators a hub can have: one that tolerates a fault.
pub const MIN_VALIDATORS: usize = 4;

/// A validator's place in its set, below [`MAX_VALIDATORS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValidatorIndex(u8);

impl ValidatorIndex {
    /// The index `index`, if a set can have it.
    #[must_use]
    pub fn new(index: usize) -> Option<Self> {
        if index < MAX_VALIDATORS {
            u8::try_from(index).ok().map(Self)
        } else {
            None
        }
    }

    /// The index as a position in the set.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

/// The consensus keys of a hub's validators, in their order in the rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorSet {
    keys: Vec<PublicKey>,
}

impl ValidatorSet {
    /// The set of `keys`, if it has between [`MIN_VALIDATORS`] and
    /// [`MAX_VALIDATORS`] distinct keys.
    #[must_use]
    pub fn new(keys: Vec<PublicKey>) -> Option<Self> {
        if keys.len() < MIN_VALIDATORS || keys.len() > MAX_VALIDATORS {
            return None;
        }
        for (i, key) in keys.iter().enumerate() {
            if keys
                .iter()
                .skip(i.saturating_add(1))
                .any(|other| other == key)
            {
                return None;
            }
        }
        Some(Self { keys })
    }

    /// n, the number of validators.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Never true: a set has at least [`MIN_VALIDATORS`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// f, the most faulty validators the set tolerates.
    #[must_use]
    pub fn faults(&self) -> usize {
        self.len().saturating_sub(1) / 3
    }

    /// q = n − f, the votes that make a certificate. For n = 3f + 1 it is
    /// 2f + 1; any two quorums share an honest validator.
    #[must_use]
    pub fn quorum(&self) -> usize {
        self.len().saturating_sub(self.faults())
    }

    /// f + 1: enough that one of them is honest.
    #[must_use]
    pub fn one_honest(&self) -> usize {
        self.faults().saturating_add(1)
    }

    /// The key of validator `index`.
    #[must_use]
    pub fn key(&self, index: ValidatorIndex) -> Option<&PublicKey> {
        self.keys.get(index.get())
    }

    /// The index of `key`, if it is in the set.
    #[must_use]
    pub fn index_of(&self, key: &PublicKey) -> Option<ValidatorIndex> {
        self.keys
            .iter()
            .position(|k| k == key)
            .and_then(ValidatorIndex::new)
    }

    /// Every validator's index and key, in order.
    pub fn iter(&self) -> impl Iterator<Item = (ValidatorIndex, &PublicKey)> {
        self.keys
            .iter()
            .enumerate()
            .filter_map(|(i, key)| ValidatorIndex::new(i).map(|index| (index, key)))
    }

    /// The keys, in order.
    #[must_use]
    pub fn keys(&self) -> &[PublicKey] {
        &self.keys
    }
}

/// Which validators of a set signed something: bit i is validator i.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Signers(u16);

impl Signers {
    /// No one.
    pub const NONE: Self = Self(0);

    /// The bitmap `bits`.
    #[must_use]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// The bitmap as bits.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether validator `index` signed.
    #[must_use]
    pub const fn contains(self, index: ValidatorIndex) -> bool {
        self.0 & (1 << index.0) != 0
    }

    /// The bitmap with validator `index` added.
    #[must_use]
    pub const fn with(self, index: ValidatorIndex) -> Self {
        Self(self.0 | (1 << index.0))
    }

    /// Everyone in either bitmap.
    #[must_use]
    pub const fn with_all(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// How many signed.
    #[must_use]
    pub const fn count(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Whether every bit names a validator of a set of `n`.
    #[must_use]
    pub const fn fits(self, n: usize) -> bool {
        n >= 16 || self.0 >> n == 0
    }

    /// The validators that signed, in order.
    pub fn iter(self) -> impl Iterator<Item = ValidatorIndex> {
        (0..MAX_VALIDATORS)
            .filter_map(ValidatorIndex::new)
            .filter(move |index| self.contains(*index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dassie_identity::NodeKey;

    fn keys(n: u8) -> Vec<PublicKey> {
        (0..n)
            .map(|i| NodeKey::from_bytes(&[i; 32]).public_key())
            .collect()
    }

    #[test]
    fn quorum_of_four_and_seven() {
        let four = ValidatorSet::new(keys(4)).unwrap();
        assert_eq!((four.faults(), four.quorum(), four.one_honest()), (1, 3, 2));
        let seven = ValidatorSet::new(keys(7)).unwrap();
        assert_eq!(
            (seven.faults(), seven.quorum(), seven.one_honest()),
            (2, 5, 3)
        );
    }

    #[test]
    fn two_quorums_share_an_honest_validator() {
        for n in MIN_VALIDATORS..=MAX_VALIDATORS {
            let set = ValidatorSet::new(keys(u8::try_from(n).unwrap())).unwrap();
            // |Q1 ∩ Q2| ≥ 2q − n, which must exceed f.
            assert!(2 * set.quorum() - n > set.faults(), "n = {n}");
        }
    }

    #[test]
    fn refuses_bad_sets() {
        assert!(ValidatorSet::new(keys(3)).is_none());
        assert!(ValidatorSet::new(keys(17)).is_none());
        let mut twice = keys(4);
        twice.push(twice[0]);
        assert!(ValidatorSet::new(twice).is_none());
    }

    #[test]
    fn bitmaps() {
        let a = ValidatorIndex::new(0).unwrap();
        let c = ValidatorIndex::new(2).unwrap();
        let signers = Signers::NONE.with(a).with(c);
        assert_eq!(signers.count(), 2);
        assert!(signers.fits(3));
        assert!(!signers.fits(2));
        assert_eq!(signers.iter().collect::<Vec<_>>(), [a, c]);
    }
}
