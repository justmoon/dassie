//! Validators' messages in batches (spec/areas/peer-protocol.md#the-batch-stream):
//! a validator sends another the messages it has waiting for it, of every
//! hub both validate, on one stream of kind
//! [`StreamKind::HubBatch`](crate::quic::StreamKind::HubBatch), rather than
//! one stream each. Each message is a frame, handed to its hub's validator
//! as one on the hub's stream would be, and each is answered by a record of
//! its own, as soon as its hub's validator replied, so one slow hub holds
//! up no other's reply.
//!
//! ```text
//! batch   = count (1 byte) || count × (hub ID (32 bytes) || length (4 bytes, big-endian) || message)
//! reply   = status (1 byte) || *record      (records only with status 0)
//! record  = frame index (1 byte) || length (4 bytes, big-endian) || answer
//! ```
//!
//! An answer is what the hub's stream replies (`HubAnswer`): its status,
//! then the messages the validator sent back. The reply's status is 0 when
//! the batch was taken, each frame then answered once, in the order its
//! answer came, and 2 when the node had no place for it: nothing was read.
//! Only a validator of a hub the node validates is read, and each frame
//! only from a validator of its own hub: a batch never carries a member's
//! request.

use std::future::Future;
use std::pin::Pin;

use dassie_hub::HubId;
use dassie_hub::digest::Digest;
use dassie_identity::NodeIdentity;

use super::HubAccess;
use super::transport::{
    FRAME_LENGTH_BYTES, HUB_ID_BYTES, HubAnswer, HubPermit, Hubs, MAX_MESSAGE_BYTES,
    MAX_REPLY_BYTES, MAX_REQUEST_BYTES, Sender,
};

/// The most frames one batch carries.
pub const MAX_BATCH_FRAMES: usize = 32;

/// A frame's bytes ahead of its message: the hub's ID and the length.
const FRAME_HEAD: usize = HUB_ID_BYTES + FRAME_LENGTH_BYTES;

/// The largest batch: its count, the largest message, and the head of
/// every frame, so that any one message fits in a batch of its own.
pub const MAX_BATCH_BYTES: usize = 1 + MAX_REQUEST_BYTES + MAX_BATCH_FRAMES * FRAME_HEAD;

/// A record's bytes ahead of its answer: the frame's index and the length.
pub const RECORD_HEAD: usize = 1 + FRAME_LENGTH_BYTES;

/// The most a batch's reply carries of answers that took a message, all
/// together: as much as one reply on the hub's stream. Past it, a frame's
/// answer is busy, and its sender sends it again.
const MAX_TAKEN_BYTES: usize = MAX_REPLY_BYTES;

/// The largest reply to a batch: its status, the answers within
/// [`MAX_TAKEN_BYTES`], and a record of every frame.
pub const MAX_BATCH_REPLY_BYTES: usize = 1 + MAX_TAKEN_BYTES + MAX_BATCH_FRAMES * (RECORD_HEAD + 1);

/// The reply's status: the batch was taken, and its records follow.
pub const BATCH_TAKEN: u8 = 0;

/// The reply's status: the node had no place for the batch, and read
/// nothing of it.
pub const BATCH_BUSY: u8 = 2;

const _: () = assert!(MAX_BATCH_FRAMES <= u8::MAX as usize);
const _: () = assert!(1 + FRAME_HEAD + MAX_MESSAGE_BYTES <= MAX_BATCH_BYTES);

/// A batch: its frames, each the hub's ID and a message.
#[must_use]
pub fn encode_batch(frames: &[(HubId, &[u8])]) -> Vec<u8> {
    assert!(
        (1..=MAX_BATCH_FRAMES).contains(&frames.len()),
        "a batch has one to {MAX_BATCH_FRAMES} frames"
    );
    let total = frames
        .iter()
        .map(|(_, message)| FRAME_HEAD.saturating_add(message.len()))
        .sum::<usize>()
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(total);
    bytes.push(u8::try_from(frames.len()).unwrap_or(u8::MAX));
    for (hub, message) in frames {
        let length = u32::try_from(message.len()).unwrap_or(u32::MAX);
        bytes.extend_from_slice(hub.0.as_bytes());
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(message);
    }
    bytes
}

/// The frames of `bytes`, if it is a batch: one to [`MAX_BATCH_FRAMES`]
/// frames that fill it, none empty or over [`MAX_MESSAGE_BYTES`], at most
/// [`MAX_BATCH_BYTES`] in all.
#[must_use]
pub fn decode_batch(bytes: &[u8]) -> Option<Vec<(HubId, &[u8])>> {
    if bytes.len() > MAX_BATCH_BYTES {
        return None;
    }
    let (count, mut rest) = bytes.split_first()?;
    let count = usize::from(*count);
    if !(1..=MAX_BATCH_FRAMES).contains(&count) {
        return None;
    }
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let (hub, after) = rest.split_first_chunk::<HUB_ID_BYTES>()?;
        let (length, after) = after.split_first_chunk::<FRAME_LENGTH_BYTES>()?;
        let length = usize::try_from(u32::from_be_bytes(*length)).ok()?;
        if length == 0 || length > MAX_MESSAGE_BYTES {
            return None;
        }
        let (message, after) = after.split_at_checked(length)?;
        frames.push((HubId(Digest(*hub)), message));
        rest = after;
    }
    rest.is_empty().then_some(frames)
}

/// A record: the frame's index, then its answer with its length ahead.
#[must_use]
pub fn encode_record(index: u8, answer: &HubAnswer) -> Vec<u8> {
    let answer = answer.encode();
    let length = u32::try_from(answer.len()).unwrap_or(u32::MAX);
    let mut bytes = Vec::with_capacity(RECORD_HEAD.saturating_add(answer.len()));
    bytes.push(index);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&answer);
    bytes
}

/// The frame's index and the length of the answer a record's head names,
/// if it is one: an answer of one to [`MAX_REPLY_BYTES`] bytes.
#[must_use]
pub fn decode_record_head(head: &[u8; RECORD_HEAD]) -> Option<(u8, usize)> {
    let (index, length) = head.split_first()?;
    let length = usize::try_from(u32::from_be_bytes(length.try_into().ok()?)).ok()?;
    (1..=MAX_REPLY_BYTES)
        .contains(&length)
        .then_some((*index, length))
}

/// What keeps a batch's reply within [`MAX_BATCH_REPLY_BYTES`]: an answer
/// that took a message, past [`MAX_TAKEN_BYTES`] of them, goes as busy.
#[derive(Debug)]
pub struct ReplyBudget {
    left: usize,
}

impl Default for ReplyBudget {
    fn default() -> Self {
        Self {
            left: MAX_TAKEN_BYTES,
        }
    }
}

impl ReplyBudget {
    /// The record of the frame at `index`, answered `answer`, within what
    /// is left.
    #[must_use]
    pub fn record(&mut self, index: u8, answer: HubAnswer) -> Vec<u8> {
        let size = answer.encode().len();
        let answer = match answer {
            HubAnswer::Taken(_) if size > self.left => HubAnswer::Busy,
            HubAnswer::Taken(messages) => {
                self.left -= size;
                HubAnswer::Taken(messages)
            }
            refused => refused,
        };
        encode_record(index, &answer)
    }
}

/// How the node takes a batch from a node, decided before anything of it
/// is read.
#[derive(Debug)]
pub enum BatchAccess {
    /// The node validates no hub with it: the stream is reset unread.
    Unknown,
    /// It does, and has no place for the batch now: it is answered busy.
    Busy,
    /// It does, and the batch is read in this place, one of validators'.
    Place(HubPermit),
}

/// What answers one frame of a batch: its index, and its hub's answer.
pub type FrameReply = Pin<Box<dyn Future<Output = (u8, HubAnswer)> + Send>>;

impl Hubs {
    /// How a batch from `from` is taken: only from a validator of a hub the
    /// node validates, in one of the places for validators' requests.
    #[must_use]
    pub fn batch_access(&self, from: &NodeIdentity) -> BatchAccess {
        if !self.validates_with(from) {
            return BatchAccess::Unknown;
        }
        match self.request_permit(HubAccess::Validator, &Sender(*from)) {
            Some(place) => BatchAccess::Place(place),
            None => BatchAccess::Busy,
        }
    }

    /// Hands each frame of `request` from `from` to its hub's validator, in
    /// order, as a message from a validator of that hub, and returns what
    /// answers each; `None` if `request` is no batch. A frame for a hub the
    /// node does not validate is answered at once as one on the hub's
    /// stream would be, and one for a hub `from` is no validator of as not
    /// one this transport takes.
    #[must_use]
    pub fn answer_batch(&self, from: Sender, request: &[u8]) -> Option<Vec<FrameReply>> {
        let frames = decode_batch(request)?;
        let replies = frames
            .into_iter()
            .zip(0_u8..)
            .map(|((hub, message), index)| -> FrameReply {
                match self.hand(&from, &hub, message, true) {
                    Ok(replied) => Box::pin(async move { (index, replied.await) }),
                    Err(answer) => Box::pin(std::future::ready((index, answer))),
                }
            })
            .collect();
        Some(replies)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(n: u8) -> HubId {
        HubId(Digest([n; 32]))
    }

    #[test]
    fn a_batch_reads_back_and_refuses_what_is_not_one() {
        let frames: Vec<(HubId, &[u8])> = vec![(hub(1), &[7, 7]), (hub(2), &[9])];
        let bytes = encode_batch(&frames);
        let mut expected = vec![2];
        expected.extend_from_slice(&[1; 32]);
        expected.extend_from_slice(&[0, 0, 0, 2, 7, 7]);
        expected.extend_from_slice(&[2; 32]);
        expected.extend_from_slice(&[0, 0, 0, 1, 9]);
        assert_eq!(bytes, expected, "the bytes are pinned");
        assert_eq!(decode_batch(&bytes), Some(frames));

        // No frames, too many, a count the bytes do not fill or that leaves
        // some over, and an empty message, are none.
        assert_eq!(decode_batch(&[0]), None);
        let mut many = vec![u8::try_from(MAX_BATCH_FRAMES + 1).unwrap()];
        for _ in 0..=MAX_BATCH_FRAMES {
            many.extend_from_slice(&[1; 32]);
            many.extend_from_slice(&[0, 0, 0, 1, 5]);
        }
        assert_eq!(decode_batch(&many), None);
        assert_eq!(decode_batch(&bytes[..bytes.len() - 1]), None);
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(decode_batch(&longer), None);
        let mut empty = vec![1];
        empty.extend_from_slice(&[1; 32]);
        empty.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(decode_batch(&empty), None);

        // The largest message fits in a batch of its own.
        let largest = vec![1; MAX_MESSAGE_BYTES];
        let one = encode_batch(&[(hub(1), &largest)]);
        assert!(one.len() <= MAX_BATCH_BYTES);
        assert_eq!(decode_batch(&one).map(|f| f.len()), Some(1));
        let over = vec![1; MAX_MESSAGE_BYTES + 1];
        assert_eq!(decode_batch(&encode_batch(&[(hub(1), &over)])), None);
    }

    #[test]
    fn a_record_names_its_frame_and_carries_its_answer() {
        let record = encode_record(3, &HubAnswer::Taken(vec![vec![5]]));
        assert_eq!(record, vec![3, 0, 0, 0, 6, 0, 0, 0, 0, 1, 5]);
        let head: [u8; RECORD_HEAD] = record[..RECORD_HEAD].try_into().unwrap();
        assert_eq!(decode_record_head(&head), Some((3, 6)));
        assert_eq!(
            HubAnswer::decode(&record[RECORD_HEAD..]),
            Some(HubAnswer::Taken(vec![vec![5]]))
        );
        assert_eq!(
            decode_record_head(&[0, 0, 0, 0, 0]),
            None,
            "no empty answer"
        );
        let over = u32::try_from(MAX_REPLY_BYTES + 1).unwrap().to_be_bytes();
        assert_eq!(
            decode_record_head(&[0, over[0], over[1], over[2], over[3]]),
            None
        );
    }

    // Answers that took messages are kept within what one reply on the
    // hub's stream holds; past it a frame is answered busy, and refusals
    // still go.
    #[test]
    fn a_reply_keeps_within_its_budget() {
        let mut budget = ReplyBudget::default();
        // Two of these fit in one reply on the hub's stream, a third not.
        let large = HubAnswer::Taken(vec![vec![1; MAX_MESSAGE_BYTES - 16]]);
        let first = budget.record(0, large.clone());
        assert_eq!(
            HubAnswer::decode(&first[RECORD_HEAD..]),
            Some(large.clone())
        );
        let second = budget.record(1, large.clone());
        assert_eq!(
            HubAnswer::decode(&second[RECORD_HEAD..]),
            Some(large.clone())
        );
        let third = budget.record(2, large);
        assert_eq!(
            HubAnswer::decode(&third[RECORD_HEAD..]),
            Some(HubAnswer::Busy)
        );
        let refused = budget.record(3, HubAnswer::UnknownHub);
        assert_eq!(
            HubAnswer::decode(&refused[RECORD_HEAD..]),
            Some(HubAnswer::UnknownHub)
        );
    }
}
