# A hub makes a block only when one is wanted, and at least every ten seconds

**Areas:** [Settlement hubs](../areas/hubs.md#consensus), [the validator's service](../areas/hubs.md#the-validators-service), [the member's side](../areas/hubs.md#packets-promises-and-claims).

## Facts

- Simplex as published starts every view's timer when a validator enters it, and a leader proposes at once: a hub that has nothing to do still makes a block every interval, a tenth of a second, 864,000 a day.
- Each block costs every pair of validators their votes, each validator a write to disk of its consensus step and of the ledger's state once the block is final, and every member the reading of a new final block. Most hubs are quiet most of the time: validators attest only what happens on the ledger, and members submit only what they pay.
- Simplex's safety rests on what an honest validator signs alone: one notarize per view, none after nullifying it, and a finalize only for a view it neither nullified nor notarized another block in. When it times out and when it proposes decide only whether the hub goes on.
- Members send their operations to every validator, and validators pass their own to every other, so whatever work there is reaches every honest validator within a message delay.
- Other things wait for blocks: the stall a validator declares, and a member judges, after a stall of no new final block (six minutes at the least); deposits, which wait for the hub to have risen within half a minute; the owner's warning after a minute without one; a hub's notices, which fall due by block stamps; and a payer whose peer never answered a packet, which may reject it upstream only once a final block stamped at or after its commitment's expiry shows no claim, within the connector's window of a second and a half.
- Ava Labs' Simplex makes the same change, and documents it as a deviation from the paper: its validators time out a view only when the application says a block should come, and the application sees that transactions reach the leader and a quorum. CometBFT does it for Tendermint (`create_empty_blocks = false`, with an interval that makes an empty block anyway).

## Decision

- **A validator arms its view's timers, and a leader proposes, only once a block is wanted:** when the application has work for one (`Application::wanted`: an operation its pool could put in a block, a member's wake), when the chain waits to be final, or once the heartbeat passed since the block the next would build on; or once the leader's proposal came, which proves it had work. Until then it sends nothing for the view, and nobody nullifies it. Once armed, a leader proposes whatever the application says later, so validators that armed on the same operation never see it fall silent. What an honest validator signs is unchanged.
- **The heartbeat is ten seconds** (`Config::heartbeat_ms`): an empty block that long after the last, so that validators and members see a quiet hub go on, far inside half a minute, a minute and the stall.
- **A chain that waits to be final wants a block** at once if the view that notarized its newest block holds more nullify votes than a quorum allows, so that block can be final only through a child; otherwise once the longest rebroadcast interval passed since its stamp. A block is final three message delays after its stamp only where messages are as fast as the region promises: a wait as short as that would chain empty blocks wherever they are slower.
- **A payer whose packet waits for the hub asks every validator for a block** (`Request::Wake`, with the commitment's expiry), again at every poll while it waits; a validator wants a block until its head is stamped at or after the latest wake it took, clamped to its clock. Anyone may ask, and the most it costs is a block every interval, as before.
- **A view's timers run from when a block became wanted, never earlier:** a validator that learnt of the work late gives the leader its whole time. Where the application always wants a block, they run from entering the view, as before.
- **Only hubs of the hub protocol's version 3 or later wait,** so validators of mixed releases never disagree on when a view times out; a release that waits takes the old behavior for older hubs.

## Accepted

- A hub's notices (a hold's reduction, new rules, a move to a newer version, a shutdown's end, an absence) fall due at the first block after them, up to a heartbeat late.
- Fees of a block wait for the votes of the blocks after it, which come only as fast as blocks do: at the heartbeat, a few minutes.
- Work that fewer than a quorum of validators hold, as an operation some refused for a full pool, waits up to a heartbeat: their timers run out, but no quorum nullifies the view. Members resubmit every second, so it is rare and short.
- Liveness now rests on an argument of our own: honest validators come to want a block within a message delay of each other, from operations every validator is sent, a chain they share, or clocks within the precision; after that the paper's argument holds. Tests cover each way a block comes to be wanted.

## Rejected

- *Slower empty blocks, every second or so:* a tenth of the cost, but a payment that comes between them waits for the next, and the margin each packet carries grows with it.
- *A heartbeat short enough for a waiting payer, without wakes:* it would have to be shorter than the margin, under a second, and save little.
- *Nullifying a view as soon as another validator does:* work that one honest validator holds would get through sooner, but any faulty validator could make the hub change views at every timeout for nothing.
- *A next-deadline scan of the ledger, so notices fall due on time:* the ledger's tables are bounded, but a scan after every block for what a heartbeat already brings within seconds is not worth its code.
- *Timers armed from the moment the work was made, as a wake's expiry:* that moment may lie before the validator learnt of it, and would cut the leader's time short.
