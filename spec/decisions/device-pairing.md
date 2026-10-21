# Only the owner on the node's machine pairs a device, with a secret kept in memory

**Areas:** [RPC](../areas/rpc.md#paired-devices), [Peer protocol](../areas/peer-protocol.md#relayed-streams), [Node](../areas/node.md#persistence), [designs/periphery](../designs/periphery.md#6-owner-apps).

## Facts

- An owner app calls the node with a device key, on a relayed stream with purpose `owner`, through a relay or dialled directly. The stream's TLS already proves the client's key to the node ([Peer protocol](../areas/peer-protocol.md#relayed-streams)), so a device needs no token of its own: its key is its credential.
- A paired device is the owner until the owner revokes it. Revoking every browser session leaves it paired.
- A browser's session is a cookie. A cookie can be stolen, and is then used from somewhere else.
- The node knows the address a WebSocket came from; it terminates TLS itself, so the address is the peer's. `Host: localhost` proves nothing: the same-origin rule accepts it from anywhere ([HTTP](../areas/http.md#same-origin-rule)).
- A core node's owner usually reaches the wallet at the node's hostname, not on `localhost`. Every owner has the IPC socket on the node's machine, which the `dassie` CLI uses.
- Pairing takes the owner minutes: show a code, scan it in the app.

## Decision

- **Only the IPC socket, or a browser that connected from a loopback address, may start a pairing.** A paired device may not. A core node's owner pairs with `dassie pair`.
- **The pairing secret lives only in memory,** for five minutes, and is claimed once. Starting another replaces it, so there is at most one; a restart forgets it.
- **The device claims the pairing with an RPC call, `ClaimPairing`, on its `owner` stream,** the one call a device that is not paired may make besides `GetBasicState`. The key its TLS proved is the key paired.
- **The node keeps a bounded list of devices, and a full list refuses a new one.** The owner revokes one to make room.

## Accepted

- **A core node's owner who only uses the wallet remotely pairs through the CLI,** or the wallet over an SSH tunnel to `localhost`.
- **A malicious process on the node's machine that can open a loopback connection and holds a session cookie can pair a device.** Such a process could read the IPC socket's owner's files anyway.
- **A pairing started and not claimed is lost on restart,** and the owner starts another.
- **The rule rests on the node seeing each client's own address.** A proxy on the node's machine that passes browsers' connections on from loopback would make every logged-in browser local, so that any session could pair a device. Nothing supports such a setup, and [HTTP](../areas/http.md#security-model) says the node must not have one; a session can already act as the owner, so a proxy would widen what a stolen session gains, not make one.

## Rejected

- *Any logged-in browser may start a pairing:* a stolen cookie, used once from anywhere, would become a device that outlives the session and every revocation of sessions, unless the owner noticed it among the devices.
- *Storing the secret, hashed:* it lives for minutes and nothing needs it after a restart; storing it adds a table and a cleanup for nothing.
- *A pairing handshake before TLS, or a separate HTTP route:* the `owner` stream already proves the key, and the RPC already has authentication per call; a second protocol would be a second thing to bound and test.
- *Dropping the oldest device when the list is full:* a device the owner relies on would stop working because someone paired another, which an attacker with a pairing could do on purpose.
