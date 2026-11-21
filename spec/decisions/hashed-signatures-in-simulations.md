# Simulations sign the hubs' votes, statements and key proofs with a hash

**Areas:** [Testing](../areas/testing.md#hashed-signatures-in-simulations), [Settlement hubs](../areas/hubs.md), [Identity](../areas/identity.md#node-key)

## Facts

- A simulation over a hub spends most of its time on Ed25519. Every validator signs what it states to each member, every member checks each statement it hears, the validators sign and check each other's votes, and a member signs a key proof each time it offers its hubs to a peer. A profile of a campaign put signing and checking node-key signatures at about two thirds of all its CPU, and nearly all of it on those messages; the peer protocol's signatures, the rate feed's and a member's operations and promises took under a percent together.
- A simulation judges what nodes do with what they receive. No node in it forges a signature: each runs Dassie's code and signs with its own key, and the faults it draws (lost, late, replayed and altered messages, Byzantine validators signing what they should not) need a signature to fail on any change and to bind its signer, not to resist a forger.
- The cryptography itself is tested apart from the simulations: the hub's golden vectors carry real signatures, and the identity crate's tests pin ZIP-215's rules and refuse weak keys.
- Simulations already replace costly crypto behind narrow interfaces ([engineering](../engineering.md#pragmatic-tigerstyle)): SLH-DSA keys come from a table ([Testing](../areas/testing.md#slh-dsa-keys-in-tests)), and XRP Ledger validations are a keyed hash ([the design](../designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation)).

## Decision

How a node key signs, and how its signatures are checked, is injected where a simulation makes and checks them by the hundred thousand.

- **`NodeSignatures`** (`dassie-identity`) signs with a node key and checks with a public key. `Ed25519Signatures` is the node key's own Ed25519. `HashSignatures` is SHA-512 over a tag, the signer's public key and the message, which the check computes again: a signature changed in any bit, over another message or of another key fails, as a real one would.
- **It is chosen per kind of message, never per call:** a hub's votes and certificates, the validators' statements to members, and the members' key proofs are signed and checked through it wherever they are signed or checked, so the signer and the checker always agree. Every other kind is signed and checked with Ed25519, in a simulation too.
- **Whatever signs or checks those messages takes it when it is made:** the consensus engine, the validator and its driver in the node, a member's hub account, which passes it to its tally of statements and to its scheme, and the node's engine core, which passes it to the hub accounts it makes. A node passes `Ed25519Signatures`, and has no setting for anything else. The simulations' harnesses pass `HashSignatures`, and so do the targeted tests built on them; every other test passes Ed25519, and the golden vectors are real.
- **The hash is keyed by the public key alone.** Making one goes through `NodeSignatures::sign`, which takes the signer's node key, so no code in a simulation makes one for a key it does not hold. A secret would add nothing.

## Accepted

- Anyone who knows a public key can make a hash it verifies. Outside a simulation it proves nothing, so a node that passed `HashSignatures` would take forgeries of these messages. Nothing in a node passes it, and nothing configures it.
- A simulation does not run Ed25519 on these messages: a failure only Ed25519 shows, such as a key the library refuses, is caught by the targeted tests and the end-to-end tests, which sign for real, not by the campaign.
- Each constructor on the way takes one more argument.

## Rejected

- *A process-wide switch to hashes,* which the simulation runner would turn on: no argument to pass anywhere, and every signature in a run hashed. But it is global state: tests in one process run side by side on threads, so one that turned it on would change the others, and nothing in a component's type says how it signs.
- *The scheme carried in the keys,* so that a key made for a simulation signs and checks by hashes: a public key decoded from the wire carries nothing that says which, so every decoder would have to be told.
- *Every kind of node signature behind the interface:* the others cost under a percent of a campaign, and each kind moved needs all its signers and checkers to take the interface. They keep Ed25519.
- *A hash keyed by a secret, as the XRP Ledger validations are:* those are signed by a fake ledger, behind servers the simulation makes lie, which must not forge one. Here every signer is a node of the simulation, which signs through the interface with its own key.
