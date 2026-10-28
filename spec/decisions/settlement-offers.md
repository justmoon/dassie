# A link state offers the schemes a node takes peers on in one list, each with its scheme's own details

**Areas:** [Peer protocol](../areas/peer-protocol.md#settlement-offers), [Network](../areas/network.md#settlement-methods), [Settlement hubs](../designs/settlement-hubs.md#11-choosing-a-hub), [Settlement hubs in the node](../areas/hubs.md#in-the-node).

## Facts

- A peer needs two things of another node's schemes before it asks to peer: which schemes it takes peers on, and, over a hub, which hub and whether it has an account there, since two members without one could never pay each other in ([the design](../designs/settlement-hubs.md#5-joining)).
- Said with an entry per scheme beside extensions for hubs, the same fact has several places: a scheme listed, and its hub named apart, by one extension for the first hub and another listing them all, with rules for which wins where they disagree, and for nodes that read one but not the other. Each place is a branch to write, read and test.
- A flag that a node takes no new peers over its hub is always set by a node that offers the hub at all: a node that takes no peers on a scheme need not offer it.
- Peers settle only over hubs ([hubs-only](hubs-only.md)), but direct settlement may come back ([the design](../designs/direct-settlement.md)), and a scheme that settles directly has things of its own to say before a peering, such as its account on the ledger.
- A link state's signature covers every byte, so a node keeps and forwards what it cannot read, and a field a node of an earlier release does not know must not make the link state unreadable to it.
- No released node speaks `dassie/2`, so its layouts may still change without a new ALPN name ([protocol-upgrades](protocol-upgrades.md)); the layout of the link state it replaces was never signed by a released node either, so it keeps its OID.

## Decision

- **One list in the link state's body, `settlement`,** after the key: each offer a scheme's ID and an octet string of the scheme's own details, in their canonical OER. Over a hub, the hub's ID and the node's account in it, 0 while it has none; on the stub, nothing. The list is bounded by the protocol (`MAX_OFFERS`, and `MAX_OFFER_DETAILS` per offer), above what a node of this release offers.
- **A scheme is offered exactly when the node takes peers on it.** No flag says otherwise: a node that wants no new peers on a scheme stops offering it, and its owner does so by removing the scheme, whose methods then wind down with `methodDrop` ([settlement-methods](settlement-methods.md)).
- **Details are read by the reader's own schemes.** A node reads another's offer only on a scheme it offers itself, as its own offer's kind says; it keeps and forwards every other offer unread. Two nodes peer only on schemes both offer, over a hub only where both name the same hub and one has an account there, in the order of the asking node's offers.
- **A broken offer is absent, not the link state.** An offer whose scheme an earlier one names, whose details are not its scheme's canonical encoding, or whose hub an offer taken before names, is taken as absent, as a known extension that breaks its rule is; the rest of the link state stands. A node's own list keeps the rules, no scheme twice, no hub twice and at most `MAX_OWN_HUBS` over hubs, which it checks before it signs.
- **The numbers left free stay reserved:** entry tag 1, which listed a scheme, and extension types 5 and 7, which named hubs, join types 2 and 6 as reserved, never given another meaning.

## Accepted

- **A node sees another's details only for schemes it runs.** A tool that reads link states learns the hubs of schemes it does not know only by reading their details as it guesses them.
- **Readers may read a broken list differently,** since which offers a node reads, and so which hub came first, depends on the schemes it offers. Each reading decides only that reader's own peering with the list's node, and an honest node never writes a broken list.
- **A length before every offer's details,** even where a scheme's details have a fixed size: a byte or three per offer, for details any node can skip.
- **No offer for a scheme a node runs but takes no peers on,** as one before its first sync, or one whose hub has stopped: another node cannot tell such a scheme from one the node does not run, which it never needs to.

## Rejected

- *Keeping the scheme entries and the hub extensions:* several places for one fact, a reading for each mix of them, and a fallback for nodes that read only some, all to test, for a layout no released node reads.
- *A list of hubs, each a scheme with its hub and account, beside the scheme entries:* the scheme still named twice, and a scheme that settles directly coming back would need a layout of its own, and so a new protocol version.
- *A typed CHOICE of offer kinds on the wire:* every node would have to know every kind to read a link state at all, so each new kind of scheme would be a new protocol version; opaque details let a node of an earlier release skip what it does not know.
- *Keeping the flag that a node takes no new peers over a hub:* always set, so a state that nothing produces and every reader must handle; taking the offer off says the same.
- *Renumbering the extensions that remain* (the periphery one and the feed port down to 2 and 3): nothing released reads `dassie/2`'s numbers, but a number that meant one thing and then another makes test data, captured link states and the code's history harder to read, for no gain: types are 16 bits.
