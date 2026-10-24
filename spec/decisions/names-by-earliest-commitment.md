# A name goes to its earliest sealed commitment, held by the majority of the bootstrap nodes' lists

**Areas:** [Names](../designs/names.md), [Network](../areas/network.md#membership), [Peer protocol](../areas/peer-protocol.md). A design, not built yet ([roadmap](../roadmap.md#names)).

## Facts

- Owners want names people can say, which a 39-character node label is not. A name has value only if every node takes the same holder for it.
- Whoever sees a registration of a name in the clear can try to register it first. A registrar sees every registration it takes.
- Every node already trusts the configured bootstrap nodes' lists, and takes a node as a member when strictly more than half of the configured bootstrap nodes list it ([membership-economics](membership-economics.md)). It needs no agreement among the bootstrap nodes for that.
- Two people may want the same name at the same time. The owners accept that either gets it, or neither; what they do not accept is losing a name to someone who learned it from their own registration.

## Decision

- **Sealed, then unsealed.** A registrant commits to a name, salted and bound to its node ID, at every registrar, and pays for each commitment. It unseals only once it holds a receipt from every registrar.
- **The earliest commitment wins** at each registrar, by the order of that registrar's own log. A registrant's priority is fixed while its name is still sealed, so no one who learns the name from the unsealing can get ahead of it, and the registrant's guarantee depends only on what was logged before its receipts.
- **A commitment's window** bounds how long it can wait to be unsealed. A registrar grants a name only once every earlier commitment's window has closed, and lists it only then.
- **The registrars are the configured bootstrap nodes,** and a name's holder is whom strictly more than half of them list, frozen when most lists are stale, as for membership.
- **A split is released.** A registrar drops a name whose holder no fresh majority confirms, so a collision that left nobody with a majority does not keep the name from everyone until it expires.
- **Fees are not refunded** for a commitment that loses or lapses.

## Accepted

- **A collision can leave a name to nobody,** until the split is released and someone registers it again.
- **A registrar that is down blocks new names** until it is back, since a registrant unseals only with every receipt. Unsealing with fewer would leave its guarantee to fewer honest registrars than a majority needs.
- **A name waits a window** before it is granted, the price of letting an earlier commitment still win.
- **A blind commitment that someone holds in reserve** wins if it is earlier, even when its holder unseals only after seeing someone else's name. It was made before the name was visible, and paid for, and lapses with its window: a prior claim, not front-running.

## Rejected

- **The first unsealing wins.** A registrant's priority would be set when it unseals, which is when its name becomes visible: the unsealing would be a race against anyone holding a commitment for the name, whom a faulty registrar could tell first. The earliest commitment sets priority while the name is still sealed, which is where an honest registrant has the advantage.
- **One ordered log among the registrars,** by consensus as a settlement hub keeps ([settlement-hubs](settlement-hubs.md)). It always names exactly one holder, but it makes the bootstrap nodes run consensus for names. The majority of lists needs nothing new, and "neither" is acceptable.
- **A single registrar.** It decides every name alone, and is trusted not to front-run anyone, which is what sealing is to make unnecessary.
- **Listing a name as soon as it is unsealed.** An earlier commitment can still take it, so a list would show a holder that may change, and tell everyone holding a commitment for the name that it is wanted.
- **Collisions as losses for both:** commitments close together in time both losing. A blind commitment could then block an honest registration it never saw.
