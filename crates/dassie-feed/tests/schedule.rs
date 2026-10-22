//! When to publish (spec/areas/rate-feed.md#publishing).

#![allow(clippy::unwrap_used, clippy::panic, clippy::unreachable)]

use dassie_feed::{FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS, FEED_VALIDITY_MS, Poll, Schedule};

const NOW: u64 = 1_790_000_000_000;

fn sequence(poll: Poll) -> u64 {
    match poll {
        Poll::Publish { sequence_ms, .. } => sequence_ms,
        Poll::Wait { .. } => panic!("expected a publish, got {poll:?}"),
    }
}

#[test]
fn the_first_poll_publishes() {
    let mut schedule = Schedule::new();
    assert_eq!(
        schedule.poll(NOW, false),
        Poll::Publish {
            sequence_ms: NOW,
            valid_until_ms: NOW + FEED_VALIDITY_MS,
            next_ms: NOW + FEED_HEARTBEAT_MS,
        }
    );
}

#[test]
fn changes_publish_at_most_once_an_interval() {
    let mut schedule = Schedule::new();
    schedule.poll(NOW, true);
    let soon = NOW + FEED_MIN_INTERVAL_MS - 1;
    assert_eq!(
        schedule.poll(soon, true),
        Poll::Wait {
            next_ms: NOW + FEED_MIN_INTERVAL_MS
        }
    );
    // The change is remembered, though this poll says nothing changed.
    assert_eq!(
        schedule.poll(soon, false),
        Poll::Wait {
            next_ms: NOW + FEED_MIN_INTERVAL_MS
        }
    );
    assert_eq!(
        sequence(schedule.poll(NOW + FEED_MIN_INTERVAL_MS, false)),
        NOW + FEED_MIN_INTERVAL_MS
    );
}

#[test]
fn without_changes_a_heartbeat() {
    let mut schedule = Schedule::new();
    schedule.poll(NOW, false);
    assert_eq!(
        schedule.poll(NOW + FEED_MIN_INTERVAL_MS, false),
        Poll::Wait {
            next_ms: NOW + FEED_HEARTBEAT_MS
        }
    );
    assert_eq!(
        sequence(schedule.poll(NOW + FEED_HEARTBEAT_MS, false)),
        NOW + FEED_HEARTBEAT_MS
    );
}

#[test]
fn a_clock_that_goes_back_does_not_hold_up_the_heartbeat_or_the_sequence() {
    let mut schedule = Schedule::new();
    schedule.poll(NOW, false);
    let earlier = NOW - 3_600_000;
    assert_eq!(
        schedule.poll(earlier, false),
        Poll::Wait {
            next_ms: earlier + FEED_HEARTBEAT_MS
        }
    );
    let poll = schedule.poll(earlier + FEED_HEARTBEAT_MS, false);
    assert_eq!(sequence(poll), NOW + 1);
    let Poll::Publish { valid_until_ms, .. } = poll else {
        unreachable!()
    };
    assert_eq!(valid_until_ms, NOW + 1 + FEED_VALIDITY_MS);
}
