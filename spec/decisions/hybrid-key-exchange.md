# Every TLS handshake offers the hybrid X25519MLKEM768 first, built on ring's X25519 and RustCrypto's ML-KEM

**Areas:** [HTTP](../areas/http.md#certificates), [Peer protocol](../areas/peer-protocol.md#quic).

## Facts

- **Confidentiality against "harvest now, decrypt later" belongs to TLS** ([peer-protocol-v1](peer-protocol-v1.md)). Every connection of a node is TLS: QUIC between nodes, the relayed streams and the tunnels, the HTTPS the wallet and Open Payments use, and the node's own requests. A key exchange of X25519 alone falls to Shor's algorithm, so a recording of any of them is read once a quantum computer runs it.
- **TLS has a standard hybrid,** X25519MLKEM768 (`0x11EC`, draft-ietf-tls-ecdhe-mlkem): the client sends an ML-KEM-768 encapsulation key (FIPS 203) and an X25519 key, the server a ciphertext and an X25519 key, and the secret is ML-KEM's shared key followed by X25519's. It holds while either holds. Chrome, Firefox, Safari and OpenSSL 3.5 offer it first by default.
- **ring is the node's only crypto provider.** Two providers in one build make rustls refuse to pick one at runtime, and ring builds without cmake for the static musl binaries, x86_64 and cross-compiled aarch64 ([Release](../areas/release.md)). ring has no ML-KEM, and rustls offers the hybrid only with its aws-lc-rs backend, a C library built with cmake.
- **A key exchange is a trait in rustls** (`SupportedKxGroup`) that any crate implements, and rustls builds its own hybrid as two groups side by side.
- **RustCrypto's `ml-kem` is pure Rust,** without `unsafe` code, checked against NIST's ACVP vectors and widely used, but never independently audited, as its README says.
- **The hybrid is large:** the client's share is 1,216 bytes, the server's 1,120. A ClientHello grows by about 1.2 KB, past the 1,200 bytes of one QUIC Initial packet.
- **TLS 1.3 signs the handshake,** the client's offer included, so a man in the middle who strips the hybrid from an offer breaks the server's signature, unless they can forge it.

## Decision

**The node's crypto provider is ring's, with X25519MLKEM768 first:** X25519MLKEM768, then ring's X25519, P-256 and P-384 (`tls::provider()` in `crates/dassie-node/src/tls.rs`). Every TLS setting of the node and of `dassie setup` takes that one provider, so all of them offer the hybrid.

- **The group is ours** (`crates/dassie-node/src/tls/x25519_mlkem768.rs`): ring's X25519 and `ml-kem`'s ML-KEM-768, laid out as the draft says, a port of rustls's own hybrid for this one group. It is for TLS 1.3 only, and refuses a share of the wrong length and an encapsulation key that fails FIPS 203's check. Its randomness comes from ring's generator, the one X25519 draws from, through two functions that take it as an argument, so known-answer vectors check them.
- **A client sends two shares,** the hybrid's and its own X25519 half's, so a server without the hybrid takes X25519 at once, without a HelloRetryRequest; the X25519 key is made once for both. A server takes the first group in the client's order that it has: a node, a browser or OpenSSL 3.5 gets the hybrid; a peer without it gets X25519.
- **Checked against another implementation:** NIST's ACVP vectors pin `ml-kem` through the group's functions (`crates/dassie-node/tests/ml_kem.rs`), and the interop test shakes hands with OpenSSL, through Node's `tls`, in both directions, with the hybrid alone and with X25519 alone (`tests/interop/tls_interop_test.ts`).

## Accepted

- **A ClientHello about 1.2 KB larger, and a server's first flight about 1.1 KB larger.** Over UDP, QUIC sends the hello in two Initial packets; a tunnel's datagrams hold it in one. The dev proxy reads a hello of up to 16 KiB ([Dev](../areas/dev.md)).
- **An ML-KEM no one has audited.** A flaw in it costs only the post-quantum half: the secret holds while X25519 holds, as it did before the hybrid, and without `unsafe` code the crate cannot corrupt the node's memory.
- **About two hundred lines of key exchange of our own,** where aws-lc-rs would have had none. They combine two implementations and split and join bytes, nothing more; they are reviewed against rustls's, and the vectors and the interop test check them.
- **`ml-kem`'s `hazmat` feature,** which documents the encapsulation that takes its randomness as an argument. The group calls it with 32 fresh bytes each time.
- **A key generation and a decapsulation per handshake** on a client, and an encapsulation on a server: well under a millisecond in optimized code.
- **A peer without the hybrid still gets X25519 alone,** and so does whoever can forge a handshake's signature and strip the hybrid from an offer. That attacker breaks the signature scheme, which is the post-quantum signing key's open point ([post-quantum-node-id](post-quantum-node-id.md)), not this one.

## Rejected

- *aws-lc-rs as the provider:* rustls's own hybrid, with no key exchange of ours, but a C library built with cmake for the static musl binaries and the aarch64 cross build, which ring was chosen to avoid. ring would stay beside it for what the node calls directly.
- *Both providers in one build:* the same build, and two C crypto libraries.
- *A crate that adds the hybrid to rustls:* the few there are are young and have few users. The code is short enough to own, on an ML-KEM that many use.
- *The hybrid alone, without the X25519 share:* every handshake with a peer, client or server without the hybrid would cost a HelloRetryRequest, a round trip.
- *ML-KEM alone:* its security would rest on lattices alone, a younger assumption than X25519's; the hybrid holds while either does.
- *SecP256r1MLKEM768, or a hybrid with ML-KEM-1024:* no common client prefers them to X25519MLKEM768, and the node's secrets rest on 128 bits anyway ([post-quantum-node-id](post-quantum-node-id.md)).
