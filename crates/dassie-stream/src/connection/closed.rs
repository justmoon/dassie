//! Which remote stream IDs are closed, in constant memory.

/// Stream indices (ID / 2) tracked individually above the base.
const WINDOW: u64 = 256;

/// Remembers which remote streams are gone, so that a late frame cannot
/// reopen one. Indices below `base` count as closed; the 256 above it are
/// tracked one bit each, and the window slides up as the remote opens
/// higher IDs. A stream skipped so far behind that the window passed it
/// counts as closed, as QUIC treats streams below the highest one opened.
/// The memory is fixed whatever the remote does
/// (spec/decisions/tigerstyle.md); the original kept no record at all, so
/// closed streams could be reopened.
#[derive(Debug, Default)]
pub(super) struct ClosedIds {
    base: u64,
    bits: [u64; 4],
}

impl ClosedIds {
    fn bit(&self, index: u64) -> bool {
        let offset = index - self.base;
        let word = usize::try_from(offset / 64).unwrap_or(usize::MAX);
        self.bits
            .get(word)
            .is_some_and(|bits| bits & (1 << (offset % 64)) != 0)
    }

    /// Whether the stream at `index` was opened before and is gone.
    pub(super) fn is_closed(&self, index: u64) -> bool {
        if index < self.base {
            true
        } else if index - self.base < WINDOW {
            self.bit(index)
        } else {
            false
        }
    }

    /// Moves the window up so that `index` is inside it.
    pub(super) fn reach(&mut self, index: u64) {
        let Some(shift) = (index + 1).checked_sub(self.base + WINDOW) else {
            return;
        };
        if shift == 0 {
            return;
        }
        if shift >= WINDOW {
            self.bits = [0; 4];
        } else {
            // At most 255 single-bit steps over four words.
            for _ in 0..shift {
                let mut carry = 0;
                for word in self.bits.iter_mut().rev() {
                    let low = *word & 1;
                    *word = (*word >> 1) | (carry << 63);
                    carry = low;
                }
            }
        }
        self.base += shift;
    }

    /// Records that the stream at `index` is gone.
    pub(super) fn mark(&mut self, index: u64) {
        if index < self.base {
            return;
        }
        self.reach(index);
        let offset = index - self.base;
        let word = usize::try_from(offset / 64).unwrap_or(usize::MAX);
        if let Some(bits) = self.bits.get_mut(word) {
            *bits |= 1 << (offset % 64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_closed_streams_and_slides() {
        let mut closed = ClosedIds::default();
        closed.mark(3);
        assert!(closed.is_closed(3));
        assert!(!closed.is_closed(2));
        closed.reach(300);
        assert_eq!(closed.base, 45);
        assert!(closed.is_closed(3), "below the window counts as closed");
        assert!(closed.is_closed(44));
        assert!(!closed.is_closed(45));
        closed.mark(299);
        assert!(closed.is_closed(299));
        assert!(!closed.is_closed(298));
        closed.reach(10_000);
        assert!(closed.is_closed(299));
        assert!(!closed.is_closed(10_000));
    }
}
