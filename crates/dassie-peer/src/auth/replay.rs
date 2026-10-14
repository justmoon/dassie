//! Replay protection for one sender (spec 7 §4.7): a timestamp window, a
//! floor, and per session a sliding window of counters, as IPsec does
//! (RFC 6479). Checking changes nothing; committing does, once the MAC has
//! verified.

/// Counters a session's window remembers below its highest.
pub const WINDOW: u64 = 1024;

/// Words of the window's bitmap.
const WORDS: usize = 16;

/// Sessions kept per sender: its current one and the one it is leaving.
const SESSIONS: usize = 2;

/// Why a message is refused as a replay, or as too old or new to tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// Its timestamp is outside the window around now.
    Stale,
    /// It is at or below the sender's floor, or was seen.
    Replayed,
}

#[derive(Clone, Copy, Debug)]
struct Session {
    id: [u8; 16],
    highest: u64,
    /// Bit `i` is set if counter `highest - i` was seen.
    seen: [u64; WORDS],
    last_ms: u64,
}

impl Session {
    fn new(id: [u8; 16], counter: u64, timestamp_ms: u64) -> Self {
        let mut seen = [0; WORDS];
        seen[0] = 1;
        Self {
            id,
            highest: counter,
            seen,
            last_ms: timestamp_ms,
        }
    }

    fn was_seen(&self, counter: u64) -> bool {
        if counter > self.highest {
            return false;
        }
        let back = self.highest - counter;
        back >= WINDOW || bit(&self.seen, back)
    }

    fn record(&mut self, counter: u64, timestamp_ms: u64) {
        if counter > self.highest {
            shift(&mut self.seen, counter - self.highest);
            self.highest = counter;
        }
        set_bit(&mut self.seen, self.highest - counter);
        self.last_ms = self.last_ms.max(timestamp_ms);
    }
}

fn bit(seen: &[u64; WORDS], back: u64) -> bool {
    let (word, offset) = position(back);
    seen.get(word).is_some_and(|w| w & (1 << offset) != 0)
}

fn set_bit(seen: &mut [u64; WORDS], back: u64) {
    let (word, offset) = position(back);
    if let Some(w) = seen.get_mut(word) {
        *w |= 1 << offset;
    }
}

fn position(back: u64) -> (usize, u32) {
    let word = usize::try_from(back / 64).unwrap_or(usize::MAX);
    let offset = u32::try_from(back % 64).unwrap_or(0);
    (word, offset)
}

/// Moves the window up by `by` counters: what was `back` is `back + by`.
fn shift(seen: &mut [u64; WORDS], by: u64) {
    if by >= WINDOW {
        *seen = [0; WORDS];
        return;
    }
    let (words, bits) = position(by);
    for index in (0..WORDS).rev() {
        let from = index.checked_sub(words);
        let high = from.and_then(|from| seen.get(from)).copied().unwrap_or(0);
        let low = from
            .and_then(|from| from.checked_sub(1))
            .and_then(|from| seen.get(from))
            .copied()
            .unwrap_or(0);
        let value = if bits == 0 {
            high
        } else {
            (high << bits) | (low >> (64 - bits))
        };
        if let Some(word) = seen.get_mut(index) {
            *word = value;
        }
    }
}

/// Where a message's session goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot(usize);

/// One sender's replay state.
#[derive(Clone, Debug)]
pub struct Guard {
    sessions: [Option<Session>; SESSIONS],
    /// Nothing at or below this timestamp is accepted.
    floor_ms: u64,
    /// The newest timestamp accepted.
    newest_ms: u64,
}

impl Guard {
    /// A guard accepting nothing at or below `floor_ms`.
    #[must_use]
    pub const fn new(floor_ms: u64) -> Self {
        Self {
            sessions: [None; SESSIONS],
            floor_ms,
            newest_ms: floor_ms,
        }
    }

    /// The newest timestamp accepted, or the floor: what to persist, so a
    /// restart refuses every message accepted before it.
    #[must_use]
    pub const fn newest_ms(&self) -> u64 {
        self.newest_ms
    }

    /// Whether every message this guard accepted is now stale, so that
    /// forgetting it opens nothing.
    #[must_use]
    pub const fn is_idle(&self, now_ms: u64, skew_ms: u64) -> bool {
        self.newest_ms.saturating_add(skew_ms) < now_ms
    }

    /// Checks a message without recording it.
    ///
    /// # Errors
    ///
    /// The message is stale or replayed.
    pub fn check(
        &self,
        now_ms: u64,
        skew_ms: u64,
        session_id: &[u8; 16],
        counter: u64,
        timestamp_ms: u64,
    ) -> Result<Slot, ReplayError> {
        if timestamp_ms > now_ms.saturating_add(skew_ms)
            || timestamp_ms < now_ms.saturating_sub(skew_ms)
        {
            return Err(ReplayError::Stale);
        }
        if timestamp_ms <= self.floor_ms {
            return Err(ReplayError::Replayed);
        }
        if let Some(index) = self.find(session_id) {
            let seen = self
                .sessions
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|session| session.was_seen(counter));
            return if seen {
                Err(ReplayError::Replayed)
            } else {
                Ok(Slot(index))
            };
        }
        Ok(Slot(self.free_slot()))
    }

    /// Records a message that [`check`](Self::check) passed and whose MAC
    /// verified.
    pub fn commit(&mut self, slot: Slot, session_id: &[u8; 16], counter: u64, timestamp_ms: u64) {
        let Some(entry) = self.sessions.get_mut(slot.0) else {
            return;
        };
        match entry {
            Some(session) if session.id == *session_id => session.record(counter, timestamp_ms),
            other => {
                // A session displaced while its messages could still be
                // replayed takes them below the floor.
                if let Some(displaced) = other {
                    self.floor_ms = self.floor_ms.max(displaced.last_ms);
                }
                *other = Some(Session::new(*session_id, counter, timestamp_ms));
            }
        }
        self.newest_ms = self.newest_ms.max(timestamp_ms);
    }

    fn find(&self, session_id: &[u8; 16]) -> Option<usize> {
        self.sessions
            .iter()
            .position(|session| session.is_some_and(|session| session.id == *session_id))
    }

    /// A free slot, or the one used longest ago.
    fn free_slot(&self) -> usize {
        self.sessions
            .iter()
            .enumerate()
            .min_by_key(|(_, session)| {
                session.map_or(0, |session| session.last_ms.saturating_add(1))
            })
            .map_or(0, |(index, _)| index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    const SKEW: u64 = 60_000;

    fn accept(guard: &mut Guard, session: u8, counter: u64, at: u64) -> Result<(), ReplayError> {
        let id = [session; 16];
        let slot = guard.check(NOW, SKEW, &id, counter, at)?;
        guard.commit(slot, &id, counter, at);
        Ok(())
    }

    #[test]
    fn each_counter_is_accepted_once() {
        let mut guard = Guard::new(NOW - SKEW);
        assert_eq!(accept(&mut guard, 1, 5, NOW), Ok(()));
        assert_eq!(accept(&mut guard, 1, 5, NOW), Err(ReplayError::Replayed));
        // Out of order, within the window.
        assert_eq!(accept(&mut guard, 1, 3, NOW), Ok(()));
        assert_eq!(accept(&mut guard, 1, 3, NOW), Err(ReplayError::Replayed));
        assert_eq!(accept(&mut guard, 1, 2000, NOW), Ok(()));
        assert_eq!(accept(&mut guard, 1, 1500, NOW), Ok(()));
        // Below the window: refused, since it could not be told apart.
        assert_eq!(accept(&mut guard, 1, 900, NOW), Err(ReplayError::Replayed));
        assert_eq!(accept(&mut guard, 1, 2000, NOW), Err(ReplayError::Replayed));
    }

    #[test]
    fn timestamps_outside_the_window_are_stale() {
        let mut guard = Guard::new(0);
        assert_eq!(
            accept(&mut guard, 1, 0, NOW + SKEW + 1),
            Err(ReplayError::Stale)
        );
        assert_eq!(
            accept(&mut guard, 1, 0, NOW - SKEW - 1),
            Err(ReplayError::Stale)
        );
        assert_eq!(accept(&mut guard, 1, 0, NOW + SKEW), Ok(()));
    }

    // A third session displaces the oldest, and its messages fall below
    // the floor, so they cannot come back as a "new" session.
    #[test]
    fn a_displaced_session_cannot_be_replayed() {
        let mut guard = Guard::new(NOW - SKEW);
        accept(&mut guard, 1, 0, NOW - 10).unwrap();
        accept(&mut guard, 2, 0, NOW - 5).unwrap();
        accept(&mut guard, 3, 0, NOW).unwrap();
        assert_eq!(
            accept(&mut guard, 1, 0, NOW - 10),
            Err(ReplayError::Replayed)
        );
        assert_eq!(
            accept(&mut guard, 1, 1, NOW - 10),
            Err(ReplayError::Replayed)
        );
        assert_eq!(accept(&mut guard, 2, 1, NOW - 4), Ok(()));
    }

    // After a restart, the persisted newest timestamp is the floor.
    #[test]
    fn a_restored_floor_refuses_what_came_before() {
        let mut before = Guard::new(NOW - SKEW);
        accept(&mut before, 1, 0, NOW + 30_000).unwrap();
        let mut after = Guard::new(before.newest_ms());
        assert_eq!(
            accept(&mut after, 1, 0, NOW + 30_000),
            Err(ReplayError::Replayed)
        );
        assert_eq!(accept(&mut after, 2, 0, NOW + 30_001), Ok(()));
    }

    #[test]
    fn the_window_shifts_by_whole_and_partial_words() {
        let mut seen = [0u64; WORDS];
        seen[0] = 0b101;
        shift(&mut seen, 70);
        assert!(bit(&seen, 70) && bit(&seen, 72) && !bit(&seen, 71));
        shift(&mut seen, 64);
        assert!(bit(&seen, 134) && bit(&seen, 136));
        shift(&mut seen, WINDOW);
        assert_eq!(seen, [0; WORDS]);
    }
}
