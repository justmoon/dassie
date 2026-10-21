# A node without a hostname keeps the default front door as its relay

**Areas:** [Node](../areas/node.md#a-chosen-relay), [HTTP](../areas/http.md#front-doors), [Payments](../areas/payments.md#the-nodes-names), [Wallet](../areas/wallet.md), [designs/periphery](../designs/periphery.md#5-reaching-a-periphery-node).

## Facts

- A node without a hostname answers Open Payments at its default name, `<label>.dassiepay.net`, from its first start, and the wallet shows that name as its payment pointer ([Payments](../areas/payments.md#the-nodes-names)).
- The front door at `dassiepay.net` reaches a node only over the connection the node keeps to it as its relay; it never dials. Finding a node through records and home nodes is not built ([roadmap](../roadmap.md#periphery)).
- So without a relay the default name answers 503: the payment pointer a new node shows reaches no one until its owner finds the front door's entry and puts it in the node's environment. Zero configuration is one of Dassie's principles ([Overview](../overview.md#principles)).
- A relay is named by its node ID, URL and key, and the connection to it is pinned to the key. The default bootstrap nodes are built into the binary the same way ([bootstrap-network](bootstrap-network.md)).
- A node with a hostname answers under it alone, so the front door of the default name does nothing for it.
- The hostname is the owner's configuration, which changes while the node runs.

## Decision

When `DASSIE_RELAY` is unset, a node's relay is the `dassiepay.net` front door (`DEFAULT_RELAY` in `crates/dassie-node/src/env.rs`, beside the default bootstrap nodes and checked as they are), and the node keeps it only while it has no hostname: it dials the front door once it has its key and no hostname, and closes the connection when a hostname is set. An entry the owner puts in `DASSIE_RELAY` is kept whether or not the node has a hostname. An empty `DASSIE_RELAY` keeps no relay, which is how an owner opts out.

Tests and the dev environment set `DASSIE_RELAY` empty, as they set their own bootstrap nodes, so that they never depend on the public front door.

## Accepted

- **Every node without a hostname keeps a connection to a machine the project runs,** today one VM. While it is down, no such node is reached at its default name. Nothing else depends on it: peering, sending, the wallet and names the owner added through another front door work without it.
- **That machine learns which nodes are online,** from which address, and since when. It also reads the Open Payments requests it passes on, as every front door does ([HTTP](../areas/http.md#front-doors)); that was already so for any owner who named it.
- **The front door's key is pinned in the binary,** so rotating it takes a release, and a node keeps dialling the old key until it updates. Rotating a front door's key already meant telling every node that named it ([HTTP](../areas/http.md#front-doors)); here the release tells them.
- **Opting out is the empty value,** which an owner has to know about. `dassie status` and the wallet name the relay a node keeps, and an onboarding is to ask ([roadmap](../roadmap.md#onboarding)).

## Rejected

- *No default:* the name a new node shows reaches no one, the wallet has to tell the owner not to hand it out, and every owner has to copy an entry from the front door's log before the node can be paid.
- *The default whether or not the node has a hostname:* a node with a hostname answers under it alone, so the connection would carry nothing, and would only tell the project's machine that the node is online.
- *The entry fetched at run time,* from DNS or dassie.land: the key would be whatever that source says, which is weaker than the signed release the bootstrap nodes already come in, and one more thing a new node waits on.
- *Asking the owner during setup:* the wallet's setup asks for a passphrase and a first ledger, and has no place to explain a relay. A step that chooses the relay, and explains what the default costs, belongs to an onboarding in the wallet ([roadmap](../roadmap.md#onboarding)); until there is one, the default makes a new node reachable.
