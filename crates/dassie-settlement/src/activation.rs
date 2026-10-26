//! What to book when a scheme starts.

/// The entry that brings the books in line with a scheme's balance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// The balances agree: nothing to book.
    InStep,
    /// The scheme's first activation: its balance is the owner's deposit,
    /// Dr `assets/settlement`, Cr `equity/owner`.
    FirstDeposit(u64),
    /// The balance grew while the node was not watching. Without knowing
    /// whose money it is, it goes to `equity/suspense`: Dr
    /// `assets/settlement`, Cr `equity/suspense`.
    Gain(u64),
    /// The balance shrank while the node was not watching: Dr
    /// `equity/suspense`, Cr `assets/settlement`.
    Loss(u64),
}

/// What to book for a scheme whose balance is `balance`, given the
/// checkpoint the books last recorded for it, if any. The original booked
/// the whole balance as a new deposit at every start.
#[must_use]
pub const fn activation(checkpoint: Option<u64>, balance: u64) -> Activation {
    match checkpoint {
        None if balance == 0 => Activation::InStep,
        None => Activation::FirstDeposit(balance),
        Some(checkpoint) if balance > checkpoint => Activation::Gain(balance - checkpoint),
        Some(checkpoint) if balance < checkpoint => Activation::Loss(checkpoint - balance),
        Some(_) => Activation::InStep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A start books only what changed, never the whole balance again.
    #[test]
    fn restarts_book_only_differences() {
        assert_eq!(activation(None, 100), Activation::FirstDeposit(100));
        assert_eq!(activation(None, 0), Activation::InStep);
        assert_eq!(activation(Some(100), 100), Activation::InStep);
        assert_eq!(activation(Some(100), 130), Activation::Gain(30));
        assert_eq!(activation(Some(100), 70), Activation::Loss(30));
        assert_eq!(activation(Some(0), u64::MAX), Activation::Gain(u64::MAX));
    }
}
