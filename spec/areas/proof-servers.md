# Proof servers

XRP Ledger servers that answer the three proof methods a node checks the ledger with ([the proof API](../designs/xrpl-validation.md#3-the-proof-api)): rippled's xrpld with a small patch, built from rippled's own sources, and one public server per network that the project runs and the node names by default.

**Paths:** `resources/proof-server/`
**Read with:** [Checking the validators](../designs/xrpl-validation.md), [verified-ledgers](../decisions/verified-ledgers.md), [XRPL](xrpl.md), [DSUSD keeper](dsusd-keeper.md#its-vm)

It is tooling ([engineering](../engineering.md#where-the-rules-bend)): a build script, a provisioning script, a boot script and xrpld's configuration, with no code of the node's. The patch is rippled's code, kept as patch files beside them.

## Never trusted

A proof server is relied on to answer, never trusted ([verified-ledgers](../decisions/verified-ledgers.md)). Everything about how it is run follows from that:

- **It holds no secret.** Its service account has no role, and it reads no secret: whoever takes it over can make it lie, which the node catches, or go silent, which stalls the node until it returns, and nothing more.
- **Its binary need not be trusted either.** The build is reproducible from pinned inputs so that anyone can build and run another server, not so that a node can check this one's binary: a node checks every answer instead.
- **Its own UNL is the published one of its network.** It decides only which ledgers the server follows; a node checks every proof against its own UNL.

## The patch

- **Small and apart from the rest of xrpld,** so that it moves to each release of rippled with a rebase:
  - in libxrpl, a branch towards any key of a tree, which proves presence and absence alike, and every validation held for a ledger, read without keeping it alive;
  - in xrpld, the three handlers.
- **In rippled's own style,** with its own unit tests, and offered upstream on rippled's `develop`. The servers run a release tag with the patches applied. Once upstream takes it, any rippled serves proofs, and owners and hub validators can name servers of their own.
- **Public and charged as other public methods are.** Each request costs as much as rippled's account queries, and `state_proof` names a bounded number of keys. So a client that floods a server gets `slowDown`, as from any rippled.
- **Its answers pin the node.** Answers of each method captured from the live servers are golden fixtures of the node's ([the design](../designs/xrpl-validation.md#13-what-pins-it)), and the fake ledger answers in the same shapes: a change to the patch's shapes fails the node's tests until the node and the fake change with it.
- **Validations are rippled's:** what its consensus code holds anyway, for about ten minutes. Serving them keeps no more of them, and keeps none of them longer. An answer holds the validations of the validators the server trusts, which are a node's own with the network's publishers. Others come only when a client asks for them (`untrusted`), and then a bounded number, the server's trusted validations first: anyone can sign validations with keys nobody trusts, and those can then crowd out only each other.

## The servers

- **One per network, on GCP,** each a VM with xrpld and Caddy, set up at every boot from the instance metadata (which build, by its hash) and from `main` (its configuration), as the [DSUSD keeper's](dsusd-keeper.md#its-vm) is. The builds are in a bucket anyone can read.
- **History.** Each keeps more than the node's history bound even right after xrpld deletes its oldest ledgers, when it holds the least, and a test of the node's reads both configs to hold the two apart. The bound is the age beyond which a node takes the server's `lgrNotFound` as history it no longer has ([the account on proofs](../designs/xrpl-validation.md#6-the-account-on-proofs)). A longer history costs disk; a shorter one sends more nodes that were away down that path.
- **Per-client limits behind the proxy.** xrpld charges each request to an address. Behind Caddy, every request would come from one address, so one client could slow down all. xrpld therefore takes the client's address from Caddy, which sets it itself and drops what a client sends there, and drops the header with which a gateway lifts the limits.
- **A testnet reset** leaves a server on a chain the network no longer has, and xrpld never takes the new chain's lower ledgers. On the testnet, a watchdog wipes the database once a server that was in sync has gone half an hour without a fresh validated ledger, and it syncs the new chain. Nodes on it stall until then and book nothing wrong. A false alarm costs the same resync, and the server's history until it is backfilled, never a wrong answer. Only an xrpld that answers and has had no fresh validated ledger for that long within one run counts, since one that does not answer or keeps crashing is systemd's to restart; and only a testnet server is ever wiped. A wipe can also be asked for in the metadata.
- **No SSH.** It logs to the serial console; a new build or configuration is new metadata and a restart, stopped cleanly so that xrpld closes its databases.

## Decisions

- [verified-ledgers](../decisions/verified-ledgers.md): proofs from a patched rippled, one server per network to start.
