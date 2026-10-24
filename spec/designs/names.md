# Names

A node is known by its ID, and in DNS by its 39-character [label](../areas/identity.md#node-label). That is fine for machines, not for people: `$alice.dassiepay.net` is an address someone can say aloud, `$<39 characters>.dassiepay.net` is not. This design lets a node's owner register a **name** on the network, held for the node's ID by the same bootstrap nodes and the same majority that decide membership ([Network](../areas/network.md#membership)), so that every core node agrees who holds it.

Registering a name is a race that anyone watching could join, so a registrant first commits to the name sealed, and pays, and reveals it only once its commitments are in place everywhere. The reasons are [names-by-earliest-commitment](../decisions/names-by-earliest-commitment.md); the design is not built yet ([roadmap](../roadmap.md#names)). Durations and sizes are starting points, to be set from the simulation when it is built.

## 1. What it guarantees

Two registrants may want the same name. Either may get it, or neither. What the protocol guarantees:

- **No front-running.** Whoever learns a name from its unsealing, a registrar included, cannot take it from the registrant who unsealed it.
- **An honest registration succeeds.** A registrant that unseals once it holds its commitments' receipts from every registrar, when no other registration of the name was in progress, holds the name, eventually, in the view of every honest core node.
- **Core nodes never disagree on a holder.** A node may not know of a name yet; it never takes a different holder for it than another honest node.

They hold while more than half of the configured bootstrap nodes are honest: with four, one may be faulty (§6).

## 2. Parties

- **Registrars** are the configured bootstrap nodes: the root every node already trusts for the node lists. A name is held when more than half of them list it, as a node is a member. A single bootstrap node, as small dev and test networks have, is a registrar alone.
- **The holder** of a name is a node ID. It need not be a member: periphery nodes, which are in no node list ([Network](../areas/network.md#periphery-nodes)), gain most from a name, and reach the registrars over QUIC like any authenticated peer request. Only the holder's own node registers and renews its names, so every message about a name is authenticated by the node it is for.

## 3. Registering

1. **Quote.** The registrant asks each registrar for the fee of one commitment. This is the quote and payment of [registration fees](../roadmap.md#registration-fees): one mechanism for both.
2. **Commit.** It picks a random salt, keeps it on disk, and sends each registrar, paid, the commitment `C = H(tag ‖ name ‖ holder ID ‖ salt)`, with a domain tag of its own. The registrar appends `C` to its **log**, with its position and the registrar's own time, persists it, and only then answers. The answer is the receipt.
   - The salt keeps a dictionary of likely names from opening a commitment.
   - The holder's ID inside `C` makes a copied commitment useless: opening it registers the name only to the node that made it.
   - A commitment the registrar already holds is answered with its receipt, and is not paid again.
3. **Unseal.** Once it holds a receipt from **every** registrar, the registrant sends each the name and the salt. An unsealing is **valid** at a registrar if it opens a commitment that registrar logged, from the node named in it, while the commitment's window is open (a set time from its logging, by the registrar's clock), and the name was free at that registrar when the commitment was logged. It is retried until the window closes.

No wait is needed between committing and unsealing: the receipts are what protect the registrant. A registrar that is down blocks the registration until it is back, since a commitment missing at one registrar is a vote the registrant cannot count on (§6). The registrant measures the window from when it sent its first commitment, which is no later than any registrar logged it, less a margin for clock rates.

## 4. The rule at each registrar

**Among the valid unsealings of a name, the commitment logged first wins.** Each registrar orders by its own log, so registrars never agree on time or order with each other, and a log's order is total, so there are no ties.

- **Priority is fixed when a commitment is logged,** while the name is still sealed. A commitment made after anyone could have learned the name is later in every honest registrar's log than the one that revealed it.
- **Granting.** An unsealed commitment can still lose to an earlier one for the same name until every earlier commitment's window has closed, which is by the end of its own window. So the registrar grants the name when the winning commitment's window closes, and lists it only then. Until then, the name is pending, and the registrar shows it to no one.
- **Lapse.** A commitment that is not unsealed by the end of its window lapses, with its fee. A commitment cannot be held as a claim for longer than its window, and the log a registrar keeps is bounded by the commitments logged within one window.
- **Order across restarts.** The log's positions and times only rise across restarts, as a link state's sequence does ([Network](../areas/network.md#the-own-link-state)), and a receipt is given only for a commitment that is on disk, so a restart never reorders the log or loses a receipted commitment.
- **Taken names.** A commitment logged while the name was held or in its grace period is void for that name.

## 5. Holding a name

- **Lists.** Each registrar serves the names it granted and has not seen expire, with their holders, as it serves its node list: polled by hash, downloaded when it changes, trusted only in a response authenticated under the registrar's configured key ([Network](../areas/network.md#node-lists-and-the-majority)).
- **The holder** of a name, in every node's view, is the node that strictly more than half of the *configured* registrars list for it. Two nodes cannot both have a majority, so honest nodes never disagree on a holder. When more than half of the lists are stale, the view freezes, as membership does.
- **Expiry and renewal.** A name expires unless its holder renews it, paid, at each registrar that lists it; no seal is needed, since the name is public. After it expires, a grace period keeps it for the holder alone, so that a renewal that reached some registrars late does not hand the name to someone else. Then it is free.
- **Releasing a split.** Registrars also poll each other's lists. A registrar that has listed a holder for a name that no majority of fresh lists confirms, for longer than registrations take to reach every registrar, while more than half of the configured lists are fresh, drops the name: it is free again at once, rather than stuck until it expires. A holder with a majority is never released, and a stale majority releases nothing.
- **Fees** are paid per commitment and per renewal, to each registrar, and grow with the size of its list, as registration's fee does ([membership-economics](../decisions/membership-economics.md)). A commitment that loses or lapses is not refunded: that is what makes a blind claim on a name cost something.
- **Bounds.** A registrar bounds the commitments in its window and the names it lists; every node bounds the names it holds, the lists it reads and the holders it computes, as it bounds its node table ([tigerstyle](../decisions/tigerstyle.md)).

## 6. Why it holds

With n configured registrars of which at most f are faulty, and n - f > n / 2 (four registrars, one faulty):

- **An honest registration succeeds.** No registration of the name was in progress when the honest registrant unsealed, so every other commitment for it was logged after the honest one, at every registrar that logged the honest one, which is every registrar. Each honest registrar therefore grants the name to the honest registrant and lists it: n - f of them, a majority. Every honest core node takes it once it has read those lists. Nothing a rival does after the receipts can change that.
- **No front-running.** Until it is unsealed, a commitment hides its name from everyone, registrars included. Whoever learns the name from the unsealing, from a faulty registrar or from a list, needs a commitment of its own, which every honest registrar logged after the honest one. It loses at n - f registrars.
- **A faulty registrar** can list whatever it likes, and grant or refuse anyone: that is one vote of n, never a majority. It cannot reorder the honest registrars' logs.
- **An earlier commitment** to the same name was a registration in progress. It wins by its priority, at every registrar that logged it earlier: that is the "one" of "one or neither", and not front-running, since it was made blind.
- **Two registrants committing at nearly the same time** may be logged in different orders at different registrars and split the vote. Then one holds the name, or neither, and a split is released (§5).

**A worked example** with four registrars R1 to R4, R4 faulty. Alice commits to `alice` at all four and holds four receipts; nobody else has a commitment for `alice`. She unseals. R4 tells Mallory, who commits to `alice` at all four, and unseals within his window. At R1 to R3, Alice's commitment is earlier: each grants `alice` to Alice when her window closes, and lists her. R4 lists Mallory. Three lists of four name Alice, so every honest node takes her as the holder. Had Bob committed to `alice` blind, a moment before Alice, R1 and R2 might log Bob first and R3 Alice: R1 and R2 list Bob, R3 Alice, R4 whomever it likes. If R4 lists Bob, Bob holds it; if not, nobody does, and R1 to R3 release the split.

## 7. Names

A name is a DNS label, so that front doors can serve it: lowercase letters, digits and hyphens, not starting or ending with a hyphen, not starting with `xn--` (no internationalized names at first), and shorter than a node label, so that no name is ever read as a label, nor a label as a name. Some names are reserved, in a list in the code. The protocol does not depend on the syntax; the code holds it.

## 8. Open questions

- **What uses a name.** Front doors serving `<name>.<domain>` for its holder, as they serve a node's label ([HTTP](../areas/http.md#front-doors)); the node answering to its names ([Payments](../areas/payments.md#the-nodes-names)); and a Dassie-aware client checking a signed answer against the holder the lists name, where it checks a label against the node ID today ([Payments](../areas/payments.md#signed-responses)). A front door is no node, so it needs the lists from the registrars itself.
- **Moving a name** to another node ID, signed by the holder, for an owner who moves to a new node.
- **The fee schedule,** with registration's ([roadmap](../roadmap.md#registration-fees)).
