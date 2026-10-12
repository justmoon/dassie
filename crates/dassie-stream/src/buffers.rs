//! Stream data buffers. Both are bounded by the policy and allocate their
//! storage once, the first time a stream uses data at all, so money-only
//! streams cost nothing.

use alloc::vec::Vec;

/// Most out-of-order fragments a receive buffer tracks. A packet whose data
/// would need more is rejected, and the sender retransmits it later.
pub const MAX_FRAGMENTS: usize = 8;

/// Outgoing stream data, kept until the remote has it.
#[derive(Debug)]
pub struct SendBuffer {
    capacity: usize,
    /// Bytes from `base_offset` on.
    bytes: Vec<u8>,
    /// Stream offset of `bytes[0]`.
    base_offset: u64,
    /// Next offset to send. Moves back when a packet with data is lost.
    next_offset: u64,
    /// Highest offset ever sent, which counts against the connection's
    /// flow-control limit.
    high_offset: u64,
}

impl SendBuffer {
    /// An empty buffer holding at most `capacity` bytes.
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            bytes: Vec::new(),
            base_offset: 0,
            next_offset: 0,
            high_offset: 0,
        }
    }

    /// Appends as much of `data` as fits and returns how much that was.
    pub fn write(&mut self, data: &[u8]) -> usize {
        if self.bytes.capacity() == 0 && self.capacity > 0 && !data.is_empty() {
            self.bytes.reserve_exact(self.capacity);
        }
        let accepted = data.len().min(self.capacity - self.bytes.len());
        self.bytes
            .extend_from_slice(data.get(..accepted).unwrap_or_default());
        accepted
    }

    /// Offset after the last byte written.
    #[must_use]
    pub fn end_offset(&self) -> u64 {
        self.base_offset + self.bytes.len() as u64
    }

    /// Next offset to send.
    #[must_use]
    pub const fn next_offset(&self) -> u64 {
        self.next_offset
    }

    /// Highest offset ever sent.
    #[must_use]
    pub const fn high_offset(&self) -> u64 {
        self.high_offset
    }

    /// The buffered bytes from `start` to `end`, which must still be held.
    #[must_use]
    pub fn slice(&self, start: u64, end: u64) -> &[u8] {
        assert!(
            start >= self.base_offset && end <= self.end_offset(),
            "data no longer buffered"
        );
        let from = usize::try_from(start - self.base_offset).unwrap_or(0);
        let to = usize::try_from(end - self.base_offset).unwrap_or(0);
        self.bytes.get(from..to).unwrap_or_default()
    }

    /// Whether any written byte is unsent.
    #[must_use]
    pub fn has_unsent(&self) -> bool {
        self.next_offset < self.end_offset()
    }

    /// Whether every written byte has been acknowledged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The unsent bytes from the next offset, at most `max_len` of them and
    /// none beyond `limit_offset`, the remote's flow-control limit.
    #[must_use]
    pub fn unsent(&self, max_len: usize, limit_offset: u64) -> (u64, &[u8]) {
        let end = self.end_offset().min(limit_offset);
        let available = end.saturating_sub(self.next_offset);
        let length = usize::try_from(available)
            .unwrap_or(usize::MAX)
            .min(max_len);
        let start = usize::try_from(self.next_offset - self.base_offset).unwrap_or(0);
        let slice = self.bytes.get(start..start + length).unwrap_or_default();
        (self.next_offset, slice)
    }

    /// Marks `length` bytes from the next offset as sent.
    pub fn mark_sent(&mut self, length: usize) {
        self.next_offset += length as u64;
        self.high_offset = self.high_offset.max(self.next_offset);
        assert!(
            self.next_offset <= self.end_offset(),
            "sent beyond written data"
        );
    }

    /// A packet carrying data from `offset` was lost: send it again.
    pub fn rewind(&mut self, offset: u64) {
        if offset >= self.base_offset && offset < self.next_offset {
            self.next_offset = offset;
        }
    }

    /// The remote has everything before `offset`: free it.
    pub fn release(&mut self, offset: u64) {
        let offset = offset.min(self.next_offset);
        if offset <= self.base_offset {
            return;
        }
        let count = usize::try_from(offset - self.base_offset).unwrap_or(0);
        self.bytes.drain(..count.min(self.bytes.len()));
        self.base_offset = offset;
    }
}

/// Incoming stream data, reassembled in order for the application.
#[derive(Debug)]
pub struct ReceiveBuffer {
    window: usize,
    /// `window` bytes once data arrives; byte `i` is stream offset
    /// `read_offset + i`.
    bytes: Vec<u8>,
    /// Offset of the next byte the application reads.
    read_offset: u64,
    /// Received ranges at or after `read_offset`, sorted and disjoint.
    fragments: [(u64, u64); MAX_FRAGMENTS],
    fragment_count: usize,
}

/// Why incoming data cannot be taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Beyond the advertised window: the remote broke flow control.
    FlowControl,
    /// Too fragmented to track now; the sender should try again.
    Fragmented,
}

impl ReceiveBuffer {
    /// An empty buffer with a `window`-byte window.
    #[must_use]
    pub const fn new(window: usize) -> Self {
        Self {
            window,
            bytes: Vec::new(),
            read_offset: 0,
            fragments: [(0, 0); MAX_FRAGMENTS],
            fragment_count: 0,
        }
    }

    fn fragments(&self) -> &[(u64, u64)] {
        self.fragments
            .get(..self.fragment_count)
            .unwrap_or_default()
    }

    /// The highest offset the remote may send up to: the window past what
    /// the application has read.
    #[must_use]
    pub fn max_offset(&self) -> u64 {
        self.read_offset + self.window as u64
    }

    /// The highest offset received so far.
    #[must_use]
    pub fn highest_offset(&self) -> u64 {
        self.fragments()
            .last()
            .map_or(self.read_offset, |&(_, end)| end.max(self.read_offset))
    }

    /// Bytes ready to read in order.
    #[must_use]
    pub fn readable(&self) -> usize {
        match self.fragments().first() {
            Some(&(start, end)) if start <= self.read_offset => {
                usize::try_from(end - self.read_offset).unwrap_or(0)
            }
            _ => 0,
        }
    }

    /// Checks whether data at `offset` of `length` bytes could be stored,
    /// with `extra_fragments` fragments already promised to other frames of
    /// the same packet.
    pub fn check(&self, offset: u64, length: usize, extra_fragments: usize) -> Result<(), Refusal> {
        let end = offset
            .checked_add(length as u64)
            .ok_or(Refusal::FlowControl)?;
        if end > self.max_offset() {
            return Err(Refusal::FlowControl);
        }
        if end <= self.read_offset || self.fragment_count + extra_fragments < MAX_FRAGMENTS {
            return Ok(());
        }
        // The table is full: the data is acceptable only if it touches an
        // existing fragment and so needs no new entry.
        let touches = self
            .fragments()
            .iter()
            .any(|&(start, stop)| offset <= stop && end >= start);
        if touches {
            Ok(())
        } else {
            Err(Refusal::Fragmented)
        }
    }

    /// Stores data at `offset`, which [`check`](Self::check) accepted.
    pub fn insert(&mut self, offset: u64, data: &[u8]) {
        let end = offset + data.len() as u64;
        assert!(end <= self.max_offset(), "data beyond the receive window");
        if end <= self.read_offset {
            return;
        }
        let start = offset.max(self.read_offset);
        let skip = usize::try_from(start - offset).unwrap_or(0);
        if self.bytes.is_empty() {
            self.bytes.resize(self.window, 0);
        }
        let position = usize::try_from(start - self.read_offset).unwrap_or(0);
        let source = data.get(skip..).unwrap_or_default();
        if let Some(target) = self.bytes.get_mut(position..position + source.len()) {
            target.copy_from_slice(source);
        }
        self.add_fragment(start, end);
    }

    fn add_fragment(&mut self, start: u64, end: u64) {
        let mut merged = (start, end);
        let mut kept = [(0u64, 0u64); MAX_FRAGMENTS];
        let mut kept_count = 0;
        for &(a, b) in self.fragments() {
            if a <= merged.1 && b >= merged.0 {
                merged = (merged.0.min(a), merged.1.max(b));
            } else if let Some(slot) = kept.get_mut(kept_count) {
                *slot = (a, b);
                kept_count += 1;
            }
        }
        assert!(kept_count < MAX_FRAGMENTS, "fragment table overflow");
        let position = kept
            .get(..kept_count)
            .unwrap_or_default()
            .iter()
            .position(|&(a, _)| a > merged.0)
            .unwrap_or(kept_count);
        self.fragment_count = 0;
        let ordered = kept.get(..position).unwrap_or_default().iter().copied();
        let after = kept
            .get(position..kept_count)
            .unwrap_or_default()
            .iter()
            .copied();
        let all = ordered.chain(core::iter::once(merged)).chain(after);
        let mut fragments = [(0u64, 0u64); MAX_FRAGMENTS];
        for (slot, fragment) in fragments.iter_mut().zip(all) {
            *slot = fragment;
            self.fragment_count += 1;
        }
        self.fragments = fragments;
    }

    /// Copies in-order data into `output`, frees it, and returns how much.
    pub fn read(&mut self, output: &mut [u8]) -> usize {
        let count = self.readable().min(output.len());
        if count == 0 {
            return 0;
        }
        if let (Some(target), Some(source)) = (output.get_mut(..count), self.bytes.get(..count)) {
            target.copy_from_slice(source);
        }
        self.bytes.copy_within(count.., 0);
        self.read_offset += count as u64;
        if let Some(first) = self.fragments.first_mut() {
            first.0 = self.read_offset;
            if first.0 >= first.1 {
                self.fragments.copy_within(1.., 0);
                self.fragment_count -= 1;
            }
        }
        count
    }

    /// Checks the buffer's internal consistency.
    pub fn check_invariants(&self) {
        let mut previous_end = self.read_offset;
        for (index, &(start, end)) in self.fragments().iter().enumerate() {
            assert!(start < end, "empty fragment");
            assert!(end <= self.max_offset(), "fragment beyond the window");
            if index == 0 {
                assert!(start >= self.read_offset, "fragment before the read offset");
            } else {
                assert!(start > previous_end, "fragments overlap or touch");
            }
            previous_end = end;
        }
    }
}
