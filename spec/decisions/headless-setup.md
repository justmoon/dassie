# An owner who keeps the passphrase on the node's host sets the node up with `dassie setup`, through the node's own setup route

**Areas:** [Node](../areas/node.md#the-dassie-command), [HTTP](../areas/http.md#setup), [Identity](../areas/identity.md#seed-paths), [overview](../overview.md#principles).

## Facts

- Setting a node up takes its owner's passphrase: the node key and the login proof derive from it, and the node takes them, with the setup token, at `POST /api/setup` ([HTTP](../areas/http.md#setup)). The wallet does that in the browser, so that the phrase never leaves it.
- Some owners keep the phrase on the node's host anyway. The market maker's two nodes are set up by a VM that nobody logs in to, and the market maker already reads each node's phrase from a file to sign its swaps ([Rate feed](../areas/rate-feed.md#market-maker)). A headless node set up by a script is the same case. For them a browser adds a person to a step that is meant to run unattended.
- The setup route checks the token, the key's form and the proof, and stores the key in the same write that deletes the token, so of two setups at once only one wins. A second way to set a node up would need all of that again.
- A node with no hostname serves a self-signed certificate that names `localhost`. No CA can vouch for it, and the IPC socket does not say which certificate the node serves.
- The node's HTTPS listener holds its port, and Linux lets only processes of the same user share a bound port. Loopback traffic never leaves the host. Whatever could answer on that port in the node's place runs as the node's user or as root, and can read the node's database, where the node key is stored in the clear anyway ([secrets-at-rest](secrets-at-rest.md)).

## Decision

- **`dassie setup --passphrase-file <file>` sets a node up from its owner's phrase, on the node's own host.** It derives the node key and the login proof as the wallet does, reads the setup token over the IPC socket, and posts them to the node's own `/api/setup` on loopback, at the HTTPS port the daemon reports. So the node checks it exactly as it checks the wallet, and the phrase never reaches the node: only the key does, as from a browser.
- **It accepts the certificate the node presents there, unverified,** checking only that the handshake is signed by that certificate's key. On loopback, at the port the daemon names over the owner's socket, nothing but the node, its own user or root can answer, and those already hold what the key would give them.
- **It is idempotent,** so a unit can run it at every start: a node set up with this phrase is left as it is, a node set up with another is refused with a code of its own and left untouched, and of the settlement schemes it is given it adds only those that are not active.
- **The phrase is read once from a bounded file and wiped as soon as the key is derived,** and no output or error names it.

## Accepted

- **The phrase lives on the node's host,** in a file the owner protects, for as long as the owner keeps it there. That is the owner's choice, made for the host where the market maker already keeps it; the principle that the phrase stays with the owner still holds, since the host is the owner's.
- **The key in the request is not wiped from every buffer** the HTTP and TLS stacks copy it into. It is what the node stores on the same host once set up; the phrase and the master seed, which can derive every other secret, are what is wiped.
- **A listener bound only to an address other than loopback cannot be set up this way.** The command says so; such an owner sets the node up in the browser, or binds loopback too.

## Rejected

- *A setup call over the IPC socket:* it would be a second path to the same state change, with its own checks, races and tests, beside one that already holds them. The socket's access is the owner's, but the HTTP route is the one the wallet proves every day.
- *Taking the certificate to pin from the node over IPC, or from its data directory:* the contract has no call that returns it, a new one would exist only for this, and reading the data directory needs the node's user. On loopback, at a port the node holds, pinning adds nothing an attacker who could answer there lacks.
- *Driving the wallet in a headless browser:* a browser on every node host, to type a phrase that is already in a file, for the same request.
- *Sending the phrase, or the master seed, to the node:* the node would then hold secrets it never needs, which derive keys outside `node/` ([Identity](../areas/identity.md#seed-paths)).
