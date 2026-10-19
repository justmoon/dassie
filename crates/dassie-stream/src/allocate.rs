//! Splitting a received amount among streams (spec/areas/stream.md#allocation).

/// One stream's claim on an incoming packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    /// The stream's shares from its `StreamMoney` frame.
    pub shares: u64,
    /// How much more the stream accepts: receive max minus received.
    pub capacity: u64,
}

/// The streams together cannot accept the amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceedsCapacity;

/// Splits `total` among `claims` in proportion to their shares, without
/// giving any stream more than its capacity, and writes the amounts into
/// `allocation` (one per claim).
///
/// This is the original's algorithm, kept because it is a deliberate
/// improvement over RFC 29's plain proportional split
/// (spec/areas/stream.md#differences-from-rfc-29-and-ilp-protocol-stream):
/// what a full stream cannot take is redistributed to the others by their
/// shares, and rounding remainders go to whoever still has room.
pub fn allocate(
    total: u64,
    claims: &[Claim],
    allocation: &mut [u64],
) -> Result<(), ExceedsCapacity> {
    assert!(allocation.len() == claims.len(), "one allocation per claim");
    allocation.fill(0);
    let mut remaining = total;
    let all_shares: u128 = claims.iter().map(|claim| u128::from(claim.shares)).sum();
    let mut ratio = (u128::from(remaining), all_shares);
    // Each round either fills a stream or hands out everything, and a round
    // without progress is followed by one that hands out at least one unit
    // per stream with room, so this bound is never reached.
    for _ in 0..2 * claims.len() + 4 {
        if remaining == 0 {
            return Ok(());
        }
        let before = remaining;
        let mut open_shares: u128 = 0;
        for (claim, allocated) in claims.iter().zip(allocation.iter_mut()) {
            let room = claim.capacity - *allocated;
            // Both factors are below 2^64, so the product fits.
            let by_share = (u128::from(claim.shares) * ratio.0)
                .checked_div(ratio.1)
                .unwrap_or(0);
            let amount = by_share.min(u128::from(room)).min(u128::from(remaining));
            // `amount` is at most `remaining`, a u64.
            let amount = u64::try_from(amount).unwrap_or(remaining);
            remaining -= amount;
            *allocated += amount;
            if amount < room {
                open_shares += u128::from(claim.shares);
            }
        }
        if remaining == 0 {
            break;
        }
        if open_shares == 0 {
            return Err(ExceedsCapacity);
        }
        ratio = if remaining == before {
            (u128::from(remaining), 1)
        } else {
            (u128::from(remaining), open_shares)
        };
    }
    assert!(remaining == 0, "allocation did not converge");
    assert!(
        allocation
            .iter()
            .map(|&amount| u128::from(amount))
            .sum::<u128>()
            == u128::from(total),
        "allocation must hand out exactly the total"
    );
    Ok(())
}
