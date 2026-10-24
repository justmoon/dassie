# Names

A node is known by its ID, and in DNS by its 39-character [label](../areas/identity.md#node-label). That is fine for machines, not for people: `$alice.dassiepay.net` is an address someone can say aloud, `$<39 characters>.dassiepay.net` is not. This design lets a node's owner register a **name** on the network, held for the node's ID by the bootstrap nodes that decide membership ([Network](../areas/network.md#membership)), so that every core node agrees who holds it.

Registering a name is a race that anyone watching could join, so a registrant first commits to the name sealed, and pays, and reveals it only once its commitments are in place everywhere. The reasons are [names-by-earliest-commitment](../decisions/names-by-earliest-commitment.md). Names, commitments and their messages are built ([Peer protocol](../areas/peer-protocol.md#names)), and so is the registrar's side of §3 to §5: its log, the rule, granting, expiry, renewal and grace, and its list ([Network](../areas/network.md#names)); the rest of the design is not built yet ([roadmap](../roadmap.md#names)). Durations and sizes are starting points, to be set from the simulation when it is built.

## 1. What it guarantees

Two registrants may want the same name. Either may get it, or neither. What the protocol guarantees:

- **No front-running.** Whoever learns a name from its unsealing, a registrar included, cannot take it from the registrant who unsealed it.
- **An honest registration succeeds.** A registrant that unseals once it holds its commitments' receipts from every registrar, when no other registration of the name was in progress, holds the name, eventually, in the view of every honest core node.
- **Core nodes never disagree on a holder.** A node may not know of a name yet, and a node whose view is frozen may still hold a name that has since expired (§5); two honest nodes with fresh views never take different holders for it.

They hold while at most f of the n configured bootstrap nodes are faulty, with n ≥ 3f + 1: with four, one may be faulty (§6). A faulty registrar can delay or block new names, though (§6).

## 2. Parties

- **Registrars** are the configured bootstrap nodes: the root every node already trusts for the node lists. A name is held when a **quorum** of them list it: n - f of the n configured registrars, where f, the faulty registrars tolerated, is the largest with n ≥ 3f + 1 (§5). A network of up to three bootstrap nodes, as dev and test networks have, tolerates none, and needs all of them.
- **The holder** of a name is a node ID. It need not be a member: periphery nodes, which are in no node list ([Network](../areas/network.md#periphery-nodes)), gain most from a name, and reach the registrars over QUIC like any authenticated peer request. Only the holder's own node registers and renews its names, so every message about a name is authenticated by the node it is for.

## 3. Registering

1. **Quote.** The registrant asks each registrar for the fee of one commitment. This is the quote and payment of [registration fees](../roadmap.md#registration-fees): one mechanism for both.
2. **Commit.** It picks a random salt, keeps it on disk, and sends every registrar at once, paid, the commitment `C = H(tag ‖ name ‖ holder ID ‖ salt)`, with a domain tag of its own ([Peer protocol](../areas/peer-protocol.md#names) has its layout). The registrar appends `C` to its **log**, with its position and the registrar's own time, persists it, and only then answers. The answer is the receipt.
   - The salt keeps a dictionary of likely names from opening a commitment.
   - The holder's ID inside `C` makes a copied commitment useless: opening it registers the name only to the node that made it.
   - A commitment the registrar already holds is answered with its receipt, and is not paid again.
3. **Unseal.** Once it holds a receipt from **every** registrar, the registrant sends each the name and the salt. An unsealing is **valid** at a registrar if it opens a commitment that registrar logged, from the node named in it, while the commitment's window is open (a set time from its logging, by the registrar's clock), and the name was free at that registrar when the commitment was logged: in that registrar's own state, neither listed nor in its grace period there, whatever other registrars list. It is retried until the window closes.

No wait is needed between committing and unsealing: the receipts are what protect the registrant. A registrar that is down, or refuses, blocks the registration, since a commitment missing at one registrar is a vote the registrant cannot count on (§6). The registrant measures the window from when it sent its first commitment, which is no later than any registrar logged it, less a margin for clock rates.

## 4. The rule at each registrar

**Among the valid unsealings of a name, the commitment logged first wins.** Each registrar orders by its own log, so registrars never agree on time or order with each other, and a log's order is total, so there are no ties. This rule is built ([Network](../areas/network.md#names)).

- **Priority is fixed when a commitment is logged,** while the name is still sealed. A commitment made after anyone could have learned the name is later in every honest registrar's log than the one that revealed it.
- **Granting.** An unsealed commitment can still lose to an earlier one for the same name until every earlier commitment's window has closed, which is by the end of its own window. So the registrar grants the name when the winning commitment's window closes, and lists it only then. Until then, the name is pending, and the registrar shows it to no one.
- **Lapse.** A commitment that is not unsealed by the end of its window lapses, with its fee. A commitment cannot be held as a claim for longer than its window, and the log a registrar keeps is bounded by the commitments logged within one window.
- **Order across restarts.** The log's positions and times only rise across restarts, as a link state's sequence does ([Network](../areas/network.md#the-own-link-state)), and a receipt is given only for a commitment that is on disk, so a restart never reorders the log or loses a receipted commitment.
- **Taken names.** A commitment logged while the name was listed or in its grace period at this registrar is void for that name. Each registrar decides by its own state: a split is resolved by releasing it (§5), not by registrars deferring to each other.

## 5. Holding a name

- **Lists.** Each registrar serves the names it granted and has not seen expire, with their holders, as it serves its node list: polled by hash, downloaded when it changes, trusted only in a response authenticated under the registrar's configured key ([Network](../areas/network.md#node-lists-and-the-majority)).
- **The holder** of a name, in every node's view, is the node that a quorum of the *configured* registrars list for it: n - f of them. A list is authenticated only to the node that fetched it, so a faulty registrar can show different nodes different lists. Any two quorums share n - 2f ≥ f + 1 registrars, at least one of them honest, which names one holder; so no faulty registrar can lead two nodes to different holders. When fewer than a quorum of the lists are fresh, the view freezes, as membership does, and may keep a holder whose name has expired since.
- **Expiry and renewal.** A name expires unless its holder renews it, paid, at each registrar that lists it; no seal is needed, since the name is public. After it expires, a grace period keeps it for the holder alone, so that a renewal that reached some registrars late does not hand the name to someone else. Then it is free.
- **Releasing a split.** Registrars also poll each other's lists. A registrar drops a name it lists when more than f of the other registrars' fresh lists lack its holder for it, for longer than registrations take to reach every registrar: the name is free again at once, rather than stuck until it expires. A holder with a quorum is missing from at most f lists, so it is never released, however late or seldom lists arrive: a list not fetched counts for nothing.
- **Fees** are paid per commitment and per renewal, to each registrar, and grow with the size of its list, as registration's fee does ([membership-economics](../decisions/membership-economics.md)). A commitment that loses or lapses is not refunded: that is what makes a blind claim on a name cost something.
- **Bounds.** A registrar bounds the commitments in its window and the names it lists; every node bounds the names it holds, the lists it reads and the holders it computes, as it bounds its node table ([tigerstyle](../decisions/tigerstyle.md)).

## 6. Why it holds

With n configured registrars of which at most f are faulty, n ≥ 3f + 1, and a quorum of n - f (four registrars, one faulty, a quorum of three):

- **An honest registration succeeds.** No registration of the name was in progress when the honest registrant unsealed, so every other commitment for it was logged after the honest one, at every registrar that logged the honest one, which is every registrar. Each honest registrar therefore grants the name to the honest registrant and lists it: n - f of them, a quorum. Every honest core node takes it once it has read those lists. Nothing a rival does after the receipts can change that.
- **No front-running.** Until it is unsealed, a commitment hides its name from everyone, registrars included. Whoever learns the name from the unsealing, from a faulty registrar or from a list, needs a commitment of its own, which every honest registrar logged after the honest one. It wins at most at the f faulty registrars, short of a quorum.
- **What a faulty registrar can do.** It cannot reorder the honest registrars' logs, and what it lists, even to each node differently, never makes a quorum for a holder that the honest registrars do not list; with up to f others it can still make a holder known to some nodes and not to others, when the honest registrars split. What it can do:
  - **Block new names.** A registrant unseals only with a receipt from every registrar, so a registrar that withholds receipts is a veto, not a vote: it can stop every new name, or those of one node, since a commitment shows who makes it. Unsealing with fewer receipts would not help: n - f receipts guarantee only n - 2f honest registrars, short of a quorum. The remedy is the configuration: a bootstrap node that misbehaves is taken off the list ([promotion-to-core](../decisions/promotion-to-core.md)).
  - **Tip off a guesser.** A commitment shows its holder and its time. Seeing a well-known node commit, a faulty registrar can have someone guess the name and race to commit at an honest registrar that has not logged the honest commitment yet, as when it is retried. Sending every commitment at once narrows that window; a guessable name of a well-known node stays exposed to it. A rival who wins one honest registrar this way splits the vote; it cannot win a quorum, since the honest registrant is earlier at the rest.
  - **Keep a split from being released,** by showing each registrar of a side its own holder confirmed, so the name stays split until it expires.
- **An earlier commitment** to the same name was a registration in progress. It wins by its priority, at every registrar that logged it earlier: that is the "one" of "one or neither", and not front-running, since it was made blind.
- **Two registrants committing at nearly the same time** may be logged in different orders at different registrars and split the vote. Then one holds the name, or neither, and a split is released (§5).

**A worked example** with four registrars R1 to R4, R4 faulty. Alice commits to `alice` at all four and holds four receipts; nobody else has a commitment for `alice`. She unseals. R4 tells Mallory, who commits to `alice` at all four, and unseals within his window. At R1 to R3, Alice's commitment is earlier: each grants `alice` to Alice when her window closes, and lists her. R4 lists Mallory. Three lists of four name Alice, so every honest node takes her as the holder. Had Bob committed to `alice` blind, a moment before Alice, R1 and R2 might log Bob first and R3 Alice: R1 and R2 list Bob, R3 Alice, R4 whomever it likes. R3 sees two lists lacking Alice and releases her. If R4 lists Bob, Bob has three of four, a quorum, and holds the name; if not, nobody does, and R1 and R2 release Bob too.

## 7. Names

A name is a DNS label, shorter than a node label, and some names are reserved: built, in [Peer protocol](../areas/peer-protocol.md#names) and `crates/dassie-peer/src/names/name.rs`. The rest of the protocol does not depend on the syntax.

## 8. Open questions

- **What uses a name.** Front doors serving `<name>.<domain>` for its holder, as they serve a node's label ([HTTP](../areas/http.md#front-doors)); the node answering to its names ([Payments](../areas/payments.md#the-nodes-names)); and a Dassie-aware client checking a signed answer against the holder the lists name, where it checks a label against the node ID today ([Payments](../areas/payments.md#signed-responses)). A front door is no node, so it needs the lists from the registrars itself.
- **Commitments that do not show who makes them,** sent and paid through another node, so that a faulty registrar can neither refuse one node's commitments nor tip off a guesser by who commits.
- **Moving a name** to another node ID, signed by the holder, for an owner who moves to a new node.
- **The fee schedule,** with registration's ([roadmap](../roadmap.md#registration-fees)).
