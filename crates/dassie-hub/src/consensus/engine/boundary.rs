//! Where the running epoch ends, on the chain being extended
//! (spec/decisions/epoch-handover.md).
//!
//! A validator learns of a change of validators from the ledger, which
//! schedules it in a block; that block may be notarized and not yet final
//! while blocks after it are proposed. So the end of an epoch is read from
//! the chain a block builds on: the application's state after its parent,
//! which the application builds as it verifies blocks, whether final or
//! not; and the changes final blocks scheduled.

use alloc::vec::Vec;

use super::super::hooks::{Application, VerifyContext};
use super::{Engine, MAX_BLOCKS};
use crate::digest::Digest;

/// The end of the running epoch on a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OnChain {
    /// The first height the running set must not certify on it, if a
    /// change is scheduled.
    Boundary(Option<u64>),
    /// A block of the chain is not held; the view to ask from.
    Missing(u64),
    /// A block of the chain is not valid to the application.
    Invalid,
}

impl Engine {
    /// The end of the running epoch on the chain that ends in `parent`, of
    /// view `view`: the earlier of the boundary the final blocks scheduled
    /// and the one the state after the block `parent` stands for names.
    pub(super) fn boundary_on(
        &mut self,
        app: &mut dyn Application,
        parent: &Digest,
        view: u64,
    ) -> OnChain {
        let Some((content, _)) = self.content_of(parent) else {
            return OnChain::Missing(view);
        };
        if let Some(problem) = self.execute(app, content, view) {
            return problem;
        }
        let known = self.epochs.boundary(self.epoch());
        // The state after the last block of the epoch before still names
        // the change that started this one.
        let started = self.epochs.running().from_height;
        let scheduled = app
            .scheduled_change(&content)
            .map(|(from_height, _)| from_height)
            .filter(|from_height| *from_height > started);
        let boundary = match (known, scheduled) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        OnChain::Boundary(boundary)
    }

    /// Has the application verify every block from the tip to `content`
    /// whose state it does not hold, in order from the nearest it holds (or
    /// the tip, whose state it always holds), so that it holds the state
    /// after `content`. A block whose state the application let go of
    /// counts as not verified: it is verified again, from the blocks held.
    fn execute(
        &mut self,
        app: &mut dyn Application,
        content: Digest,
        view: u64,
    ) -> Option<OnChain> {
        let mut path = Vec::new();
        let mut at = content;
        let mut asked = view;
        for _ in 0..=MAX_BLOCKS {
            if at == self.tip_digest {
                break;
            }
            if self.executed.contains(&at) {
                if app.holds(&at) {
                    break;
                }
                self.executed.remove(&at);
            }
            let Some(block) = self.blocks.get(&at) else {
                return Some(OnChain::Missing(asked));
            };
            path.push((at, block.clone()));
            asked = block.parent_view;
            at = block.parent;
        }
        for (digest, block) in path.into_iter().rev() {
            let Some(proposer) = self.leader(block.view) else {
                return Some(OnChain::Invalid);
            };
            let context = VerifyContext {
                view: block.view,
                proposer,
                parent: block.parent,
                height: block.height,
                timestamp_ms: block.timestamp_ms,
                block: digest,
            };
            if !app.verify(&context, &block.payload) {
                return Some(OnChain::Invalid);
            }
            self.executed.insert(digest);
        }
        None
    }
}
