# D18. One grace limit per peer

**Status:** superseded by [D23](D23-each-node-books-its-own-fulfills.md) on 2026-10-18. Decided 2026-10-14, with numbers set 2026-10-15.

## What it decided

Each peer had one grace limit, a share of the credit we extend (75% by default). Everything a peer cost us in a rolling window counted against it: Fulfills it refused ([D17](D17-the-upstream-decides-whether-a-fulfill-counted.md)), and debt it left past the point where it should have settled for longer than an hour. A peer past its limit was banned automatically.

## Why it was replaced

Under [D20](D20-trust-follows-what-a-peer-earned-us.md) a senior extends its junior nothing, so the limit fell to its floor, and the simulation banned honest juniors for the Fulfills one of their crashes made them refuse. Both things it counted are already bounded: a peer can never owe more than the credit we extend, and once each node keeps its own Fulfills, a refusal changes nothing in our books.

## What remains

- A peer costs us at most the credit we extend, plus what it has in flight.
- Only the owner bans. Bans a node made for a grace limit were lifted on upgrade, and the owner's grace share setting went away.
- The owner's group takes in what the node gains or loses, which came with this decision and stays ([D6](D06-owner-ledger-and-multi-currency-accounts.md)).
