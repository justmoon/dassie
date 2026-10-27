# A receive limit alone bounds nothing: it is refused

**Areas:** [Payments](../areas/payments.md).

## Facts

- Open Payments is changing the limits of an outgoing-payment grant: a debit limit, or a debit limit and a receive limit, but never a receive limit alone.
- A receive limit counts what payments deliver. What they debit for it depends on the path's rate and fees, which the client steers through the receivers it pays. So a grant capped only by what is delivered puts no bound on what the owner pays.

## Decision

A receive limit alone is refused when a grant is asked for.

- **A payment under such limits is refused** when it is made, whatever the grant's state, so a store from elsewhere, as a backup put back, pays nothing under one either.

## Accepted

- Apps on the Open Payments SDK can ask only for a debit limit until the SDK accepts both together.

## Rejected

- *Add a debit limit derived from the receive limit:* it would cap at a rate the owner never saw. The owner approved what is delivered, not what is paid.
