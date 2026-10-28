# A withdrawal is sized as the hub scheme pays it, and a failed move backs off only its own way

**Areas:** [Settlement hubs](../areas/hubs.md#in-the-node)

## Facts

- The node keeps its hub account between a floor and a ceiling: below the floor it deposits from the owner book, above the ceiling it withdraws to the owner's own account, never below the floor. It decided both from the account's agreed state, which the validators say only every few seconds.
- The node's own settlements, hold fundings, take from the balance as each is queued and as the hub executes it, and the hub scheme books each as it executes. The scheme pays an owner's withdrawal only from what the account can spend once the operations ahead of it executed: the newest state's balance, never more than the books hold of it, less what the operations waiting or on their way take.
- So a withdrawal sized from the agreed state could be more than the scheme would pay. On the `peer-swaps` e2e flow, a hold funding of 1.26 XRP executed 0.4 s before a funding check; the state still showed 5.51 XRP, and the node sized a withdrawal of 4.49 XRP down to the floor. The scheme refused it, since the account could spend 4.25 XRP.
- A move that failed, or could not be made, held back the next move either way for a backoff of 60 s, doubling. Hold fundings then took the balance below the floor, to nothing, and the deposit the floor called for waited out the rest of the backoff: the hub sat dry for 39 s, and every request to settle on it in that time went unfunded.
- Larger hold fundings, as when a request funds more for a peer that asks often, make the race more likely.
- The backoff is there so a side that fails is not asked again at every check. A deposit is paid from the XRPL account and a withdrawal from the hub: one failing says nothing of the other.

## Decision

- **The scheme says what a withdrawal could take** (`SettlementScheme::owner_spendable`), as its `pay_owner` checks it, the payment's fees included. The node sizes the withdrawal from that, never from more than the agreed state's balance, and leaves the floor and what the owner's unfinished payments need; an owner's withdrawal beyond it is refused with that reason. A withdrawal the node makes is one the scheme takes, unless the account cannot queue it.
- **A failed move backs off only the moves its way.** Deposits and withdrawals each have a backoff of their own, from 60 s, doubling to an hour, and back to the first wait once a move that way is done. A withdrawal refused or failed never keeps the floor's deposit from going, nor a failed deposit a withdrawal past the ceiling. A token hub's gas check keeps its own as before.
- The floor's check stays on the agreed state: a deposit decided from what the scheme can spend could go twice, while the books have not yet booked a deposit the state shows.

## Accepted

- **A withdrawal past the ceiling may wait for the next state** while hold fundings are on their way: the node keeps a little more in the hub for a few seconds rather than withdraw what those fundings need.
- **A side that keeps failing is still asked once per backoff,** each side on its own: a hub that refuses withdrawals and an XRP Ledger that fails deposits are each tried at most once a minute at first, as before.
- **A trait method more,** which defaults to telling nothing; only the hub scheme pays an owner's payment from a balance its own operations draw on.

## Rejected

- *Size the withdrawal from the agreed state, and size it again when the scheme refuses it:* the node would guess at what the scheme counts from a refusal that does not say, and still try a payment it can tell beforehand will fail.
- *No backoff for a move the scheme refuses before anything is sent, only for one a ledger failed:* a refusal that persists, as a full queue, would then be tried at every check; one backoff per way covers both causes.
- *Decide the floor's deposit from what the scheme can spend too,* so a deposit leaves as soon as fundings are queued: right after a deposit lands, the state shows it before the books do, and the floor could call for a deposit the hub already credited.
