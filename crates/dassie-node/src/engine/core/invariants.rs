//! What holds between steps, checked before each commit (D16): every
//! state machine the core owns checks its own, and the core checks how
//! they fit together. The parts whose fields a module keeps to itself are
//! checked in that module.

use super::Core;

impl Core {
    pub(super) fn check_invariants(&self) {
        self.ledger.check_invariants();
        self.connector.check_invariants(&self.ledger);
        self.peer.check_invariants();
        for active in self.schemes.values() {
            active.scheme.check_invariants();
        }
        self.check_receivers();
        self.check_incoming();
        self.check_outgoing();
        self.check_probes();
        self.owed.check();
    }
}
