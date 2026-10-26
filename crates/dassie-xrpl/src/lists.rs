//! When each publisher's validator list is fetched from its sites, and
//! what an answer does to the UNL
//! (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers),
//! sans-IO: the schedule the account and the standalone verifier share
//! ([`crate::verifier`]).
//!
//! - **A fetch** is an HTTPS GET of a publisher's current site, which its
//!   holder carries out and answers with the body, or nothing on any
//!   failure. A fetch is due when the publisher's list is to be fetched
//!   again ([`LIST_REFRESH_MS`] at most, half the time to its expiry near
//!   it), and at once when there is no UNL or quorum was lost
//!   ([`ListFetches::hasten`]).
//! - **An answer** is every publisher's that asked that site, since two
//!   publishers may share one; the list in it is taken for the publisher it
//!   is by its key, and the others' fetch failed. A site that fails is
//!   followed by the publisher's next at once, and after the last the first
//!   again [`LIST_RETRY_MS`] later. A fetch unanswered after
//!   [`FETCH_TIMEOUT_MS`] failed.

use alloc::string::String;
use alloc::vec::Vec;

use crate::keys::PublicKey;
use crate::unl::{RIPPLE_EPOCH_UNIX_S, Taken, Unl};
use crate::validator_list::{ListError, Message, UnlConfig};

/// How long the node may take to fetch a list.
pub const FETCH_TIMEOUT_MS: u64 = 30_000;
/// How long after a publisher's sites all failed they are asked again.
pub const LIST_RETRY_MS: u64 = 60_000;
/// How often a list is fetched again, at most: sooner near its expiry.
pub const LIST_REFRESH_MS: u64 = 3_600_000;

/// One publisher's list, being fetched or waiting to be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Fetching {
    /// The site asked next, by its place in the publisher's.
    site: usize,
    /// When the fetch in flight was asked for.
    asked_ms: Option<u64>,
    /// When it is fetched next.
    next_ms: u64,
    /// Whether every site failed when it was last fetched, so that it waits
    /// [`LIST_RETRY_MS`] before the next.
    failed: bool,
}

/// Each publisher's fetch, in the config's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListFetches {
    fetches: Vec<Fetching>,
}

/// What an answer to a fetch did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answered {
    /// How many times what the UNL keeps changed: lists that expired or
    /// took effect before the answer was read, and the list it took.
    pub changes: u64,
}

impl ListFetches {
    /// The fetches of `publishers` publishers, each due at once.
    #[must_use]
    pub fn new(publishers: usize) -> Self {
        Self {
            fetches: alloc::vec![Fetching::default(); publishers],
        }
    }

    /// The sites to fetch now: each publisher's whose fetch is due and none
    /// is in flight, each asked from now on.
    pub fn due(&mut self, config: &UnlConfig, now_ms: u64) -> Vec<String> {
        let mut asks = Vec::new();
        for (fetching, publisher) in self.fetches.iter_mut().zip(&config.publishers) {
            if fetching.asked_ms.is_some() || fetching.next_ms > now_ms {
                continue;
            }
            fetching.failed = false;
            let Some(url) = publisher.sites.get(fetching.site) else {
                fetching.site = 0;
                continue;
            };
            fetching.asked_ms = Some(now_ms);
            asks.push(url.clone());
        }
        asks
    }

    /// Every publisher's list is due at once, where none is in flight and
    /// its sites did not all fail within [`LIST_RETRY_MS`]: quorum was
    /// lost, or there is no UNL.
    pub fn hasten(&mut self, now_ms: u64) {
        for fetching in &mut self.fetches {
            if fetching.asked_ms.is_none() && !fetching.failed {
                fetching.next_ms = fetching.next_ms.min(now_ms);
            }
        }
    }

    /// A fetch of `url` answered with `body`, or failed: it answers every
    /// publisher that asked that site, and the list in it, taken into
    /// `unl`, is a publisher's own by its key; `None` if no publisher asked
    /// that site. Lists that expired or took effect are moved first, so an
    /// expired one is no list, nor the same one again.
    pub fn answered(
        &mut self,
        unl: &mut Unl,
        config: &UnlConfig,
        (url, body): (&str, Option<&[u8]>),
        now_ms: u64,
    ) -> Option<Answered> {
        let asked: Vec<usize> = (self.fetches.iter().zip(&config.publishers).enumerate())
            .filter(|(_, (f, p))| {
                f.asked_ms.is_some() && p.sites.get(f.site).is_some_and(|site| site == url)
            })
            .map(|(publisher, _)| publisher)
            .collect();
        if asked.is_empty() {
            return None;
        }
        let mut changes = u64::from(tick(unl, now_ms));
        let taken = body.map(|body| unl.take_list(body, now_ms));
        if matches!(taken, Some(Ok(taken)) if taken != Taken::Same) {
            changes = changes.saturating_add(1);
        }
        // A list of a publisher with as many waiting as are kept came from
        // its site all the same: the site served, and nothing new was
        // taken.
        let served = matches!(taken, Some(Ok(_) | Err(ListError::TooManyPending)));
        let by = (body.filter(|_| served))
            .and_then(|body| Message::parse(body).ok())
            .map(|message| message.publisher);
        for publisher in asked {
            self.fetched_for(unl, config, publisher, by, now_ms);
        }
        Some(Answered { changes })
    }

    /// Publisher `publisher`'s fetch answered, with a list taken of the
    /// publisher `by`, if any: fetched again later if it is its own, else
    /// from its next site.
    fn fetched_for(
        &mut self,
        unl: &Unl,
        config: &UnlConfig,
        publisher: usize,
        by: Option<PublicKey>,
        now_ms: u64,
    ) {
        let publishers = &config.publishers;
        let own = publishers
            .get(publisher)
            .is_some_and(|p| by == Some(p.master_key));
        let next = own.then(|| now_ms.saturating_add(refresh_after(unl, publisher, now_ms)));
        let sites = publishers.get(publisher).map_or(1, |p| p.sites.len());
        if let Some(fetching) = self.fetches.get_mut(publisher) {
            fetching.asked_ms = None;
            if let Some(next) = next {
                fetching.next_ms = next;
                fetching.failed = false;
            } else {
                // The next site at once; after the last, a while later.
                fetching.site = fetching.site.saturating_add(1) % sites.max(1);
                fetching.failed = fetching.site == 0;
                fetching.next_ms = if fetching.failed {
                    now_ms.saturating_add(LIST_RETRY_MS)
                } else {
                    now_ms
                };
            }
        }
    }

    /// The publishers whose fetch took too long, and failed: each is
    /// answered with nothing at its site ([`Self::site`],
    /// [`Self::answered`]).
    #[must_use]
    pub fn late(&self, now_ms: u64) -> Vec<usize> {
        (self.fetches.iter().enumerate())
            .filter(|(_, f)| {
                f.asked_ms
                    .is_some_and(|at| at.saturating_add(FETCH_TIMEOUT_MS) <= now_ms)
            })
            .map(|(publisher, _)| publisher)
            .collect()
    }

    /// The site publisher `publisher` is asked at, or next.
    #[must_use]
    pub fn site(&self, config: &UnlConfig, publisher: usize) -> Option<String> {
        let site = self.fetches.get(publisher)?.site;
        config.publishers.get(publisher)?.sites.get(site).cloned()
    }

    /// When a fetch is due next, or one in flight is late.
    #[must_use]
    pub fn deadline(&self) -> Option<u64> {
        (self.fetches.iter())
            .map(|f| match f.asked_ms {
                Some(at) => at.saturating_add(FETCH_TIMEOUT_MS),
                None => f.next_ms,
            })
            .min()
    }

    /// The fetches in flight are forgotten: their answers come on a
    /// connection gone.
    pub fn forget(&mut self) {
        for fetching in &mut self.fetches {
            fetching.asked_ms = None;
        }
    }

    /// Whether any fetch is in flight.
    #[must_use]
    pub fn any_asked(&self) -> bool {
        self.fetches.iter().any(|f| f.asked_ms.is_some())
    }

    /// Asserts what holds of them, for the publishers of `config`.
    pub fn check_invariants(&self, config: &UnlConfig) {
        assert_eq!(
            self.fetches.len(),
            config.publishers.len(),
            "one fetch for each publisher"
        );
        for (fetching, publisher) in self.fetches.iter().zip(&config.publishers) {
            assert!(
                fetching.site < publisher.sites.len(),
                "the site asked next is the publisher's"
            );
            assert!(
                !(fetching.failed && fetching.asked_ms.is_some()),
                "a fetch is asked only once its sites may be asked again"
            );
        }
    }
}

/// Lists in `unl` that expired go, and those that took effect are current:
/// whether any publisher's lists changed.
pub fn tick(unl: &mut Unl, now_ms: u64) -> bool {
    let before = sequences(unl);
    unl.tick(now_ms);
    sequences(unl) != before
}

/// Each publisher's current and pending lists' sequences.
fn sequences(unl: &Unl) -> Vec<(Option<u32>, Vec<u32>)> {
    (0..unl.config().publishers.len())
        .map(|publisher| {
            let (current, pending) = unl.lists(publisher);
            let pending = pending.iter().map(|l| l.sequence).collect();
            (current.map(|l| l.sequence), pending)
        })
        .collect()
}

/// How long until publisher `publisher`'s list is fetched again: at most
/// [`LIST_REFRESH_MS`], and half the time to its expiry, so that it is
/// fetched more often near it.
fn refresh_after(unl: &Unl, publisher: usize, now_ms: u64) -> u64 {
    let (current, _) = unl.lists(publisher);
    let expires_ms = current.map_or(0, |list| {
        (u64::from(list.expiration) + RIPPLE_EPOCH_UNIX_S).saturating_mul(1_000)
    });
    let half = expires_ms.saturating_sub(now_ms) / 2;
    half.clamp(LIST_RETRY_MS, LIST_REFRESH_MS)
}
