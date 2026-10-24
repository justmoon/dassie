# A receive limit alone bounds nothing: it is refused, and grants kept with one are revoked

**Areas:** [Payments](../areas/payments.md).

## Facts

- Open Payments is changing the limits of an outgoing-payment grant: a debit limit, or a debit limit and a receive limit, but never a receive limit alone.
- A receive limit counts what payments deliver. What they debit for it depends on the path's rate and fees, which the client steers through the receivers it pays. So a grant capped only by what is delivered puts no bound on what the owner pays.
- The node checks limits when a grant is asked for. A grant approved before the rule was never checked against it, and nothing checks it again: its client kept paying under it.
- A grant the owner revokes loses its tokens at once. Its client, continuing it, is told it was not approved, and must ask again, which needs the owner again. The owner no longer sees it in Connected Apps, and it is forgotten at the next pruning.
- The store upgrades its schema once, in a transaction, before the node serves anything ([database](../areas/node.md#database)).

## Decision

A receive limit alone is refused when a grant is asked for, and a grant kept from before with one is revoked when the node upgrades.

- **The upgrade revokes it as the owner would:** a grant, pending or approved, with an item that creates outgoing payments under a receive limit and no debit limit loses its tokens and is marked revoked, in the migration that upgrades the store.
- **A payment under such limits is refused** when it is made, whatever the grant's state, so a store from elsewhere, as a backup put back, pays nothing under one either.
- **Only what can pay goes.** A grant whose items with a receive limit alone only read or list keeps its access: there the limit bounds no payment.

## Accepted

- Clients that hold such a grant lose it without notice and must ask again, and the owner must approve again. Apps on the Open Payments SDK can ask only for a debit limit until the SDK accepts both together.
- The owner sees the grants leave Connected Apps and is not told why.
- A pending grant revoked so counts as decided when it was asked for, since a migration has no clock. Nothing reads when a revoked grant was decided.
- A read-only item keeps a receive limit alone, which a request could not ask for now.

## Rejected

- *Keep them, as when the rule came:* each such grant still debits without bound, and its owner approved it under a rule the node no longer holds.
- *Refuse their payments only:* the owner keeps seeing an approved grant that can never pay, and its client a token that works for all else.
- *Add a debit limit derived from the receive limit:* it would cap at a rate the owner never saw. The owner approved what is delivered, not what is paid.
- *Revoke them in code at every start:* a migration runs once, in the upgrade's transaction, before anything serves, and the refusal at payment time covers what it cannot reach.
