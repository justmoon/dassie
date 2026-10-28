# An owner who keeps the passphrase on the node's host sets the node up with `dassie setup`, through the node's own setup route

**Areas:** [Node](../areas/node.md#the-dassie-command), [HTTP](../areas/http.md#setup), [Identity](../areas/identity.md#seed-paths), [overview](../overview.md#principles).

## Facts

- Setting a node up takes its owner's passphrase: the node key derives from it, and the node takes it, with the setup token, at `POST /api/setup` ([HTTP](../areas/http.md#setup)). The wallet does that in the browser, so that the phrase never leaves it.
- Some owners keep the phrase on the node's host anyway. The market maker's two nodes are set up by a VM that nobody logs in to, and the market maker already reads each node's phrase from a file to sign its swaps ([Rate feed](../areas/rate-feed.md#market-maker)). A headless node set up by a script is the same case. For them a browser adds a person to a step that is meant to run unattended.
- The setup route checks the token and the key's form, and stores the key in the same write that deletes the token, so of two setups at once only one wins. A second way to set a node up would need all of that again.
- A node with no hostname serves a self-signed certificate that names `localhost`. No CA can vouch for it, and anyone can make one that names `localhost`.
- The node does not always hold every loopback address at its HTTPS port. A node that listens on one address (`--listen`), or on IPv6 alone where the host keeps IPv6 sockets apart from IPv4, leaves the other loopback address free at that port, and any user of the host can bind it.
- The daemon's status, over the IPC socket, names the certificate it serves. Its key is in the node's database, beside the node key, which is stored in the clear there anyway ([secrets-at-rest](secrets-at-rest.md)).

## Decision

- **`dassie setup --passphrase-file <file>` sets a node up from its owner's phrase, on the node's own host.** It derives the node key as the wallet does, reads the setup token over the IPC socket, and posts them to the node's own `/api/setup` on loopback, at the HTTPS port the daemon reports. So the node checks it exactly as it checks the wallet, and the phrase never reaches the node: only the key does, as from a browser.
- **The realm is the phrase's.** A phrase names its realm ([phrase-format](phrase-format.md)), so `dassie setup` derives the key in it and names it in the request, and takes no flag for it. The node checks it as it checks the wallet's, so a node that may not run the live realm refuses a live phrase, and one set up in the other realm refuses it too.
- **It pins the certificate the daemon names,** by the SHA-256 of its leaf, and requires the handshake to be signed by that certificate's key. A listener at a loopback address that cannot do that is passed over and sent nothing. Only the node, or whoever can read its database and so the node key too, can answer as the node.
- **It is idempotent,** so a unit can run it at every start: a node set up with this phrase is left as it is, a node set up with another is refused with a code of its own and left untouched, an external asset's config it is given is set only if it differs, and of the settlement schemes it is given it adds only those that are not active.
- **The phrase is read once from a bounded file and wiped as soon as the key is derived,** and no output or error names it.
- **One flag says which settlement hubs it joins, the same in either realm:** `--hub <scheme>`, repeatable, names each by the scheme over it, as `dassie hub` names a hub, and joins the default hub of that scheme's asset; `--hub none` joins none. Without it, each realm keeps its default: the default hub of XRP on the testnet, none in the live realm, where a node joins a hub only once its owner turns hubs on ([Node](../areas/node.md#the-dassie-command)). A hub it was told to join and cannot fails setup, so that a script never takes the exit for a hub joined.

## Accepted

- **The phrase lives on the node's host,** in a file the owner protects, for as long as the owner keeps it there. That is the owner's choice, made for the host where the market maker already keeps it; the principle that the phrase stays with the owner still holds, since the host is the owner's.
- **The key in the request is not wiped from every buffer** the HTTP and TLS stacks copy it into. It is what the node stores on the same host once set up; the phrase and the master seed, which can derive every other secret, are what is wiped.
- **A listener bound only to an address other than loopback cannot be set up this way.** The command says so; such an owner sets the node up in the browser, or binds loopback too.

## Rejected

- *A setup call over the IPC socket:* it would be a second path to the same state change, with its own checks, races and tests, beside one that already holds them. The socket's access is the owner's, but the HTTP route is the one the wallet proves every day.
- *Accepting whatever certificate answers on loopback at the node's port:* safe only while the node holds every loopback address at that port. Another user who binds the address the node leaves free would be sent the node key and the login proof.
- *Taking the certificate to pin from the node's data directory:* reading it needs the node's user, and the status already reads the certificate the node serves.
- *A `--realm` flag:* the phrase says its realm, so a flag could only agree with it or have the phrase refused, and every provisioning script would have to name, in a second place, what the phrase already says.
- *A flag per realm, `--no-hub` to join none on the testnet and a bare `--hub` to join one in the live realm:* the same intent took a different flag in each realm, so a script had to know the realm the phrase names, and `--hub` could mean only the default hub of XRP, though a node is in one hub per asset and the testnet has a DSUSD hub too.
- *Driving the wallet in a headless browser:* a browser on every node host, to type a phrase that is already in a file, for the same request.
- *Sending the phrase, or the master seed, to the node:* the node would then hold secrets it never needs, which derive keys outside `node/` ([Identity](../areas/identity.md#seed-paths)).
