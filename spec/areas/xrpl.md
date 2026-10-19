# XRPL

The `xrpl-testnet` settlement scheme: the node's wallet on the XRP Ledger testnet, derived from its passphrase; the Payments it signs to settle; how it follows its account and books every transaction that touches it once; how it survives restarts; and the gates that keep real money out.

**Paths:** `crates/dassie-xrpl/`, `crates/dassie-node/src/fake_ledger.rs`, `testdata/xrpl/`
**Read with:** [Settlement](settlement.md), [Ledger](ledger.md), [Identity](identity.md)

The crate does no IO and reads no clock. The scheme says what to send its ledger (`LedgerSend`) and books what the ledger says, through the node, over the connection the node keeps for it ([Settlement](settlement.md#schemes-that-follow-a-ledger)). Its constants are in `lib.rs` and `scheme.rs`.

## Real money

Real money is refused at every level until the conditions of [D8](../decisions/D08-real-money.md) hold, which the [roadmap](../roadmap.md#real-money) lists. This scheme meets the XRP Ledger's part of them (the reserve is never booked, an account's creation is a deposit, it settles whole drops, books fees and checks results and senders); mainnet also needs a memo type of its own. Until then:

- The realm `live` is refused ([Node](node.md)), a scheme on a ledger outside the test realm does not activate ([Settlement](settlement.md#schemes-and-ledgers)), and there is no mainnet scheme.
- **The testnet scheme refuses other networks.** Before it reads its account, it checks that its server reports the testnet's network ID. A server that reports another, or none, stops the scheme as failed, before anything is booked, written off or signed. An owner who points it at a mainnet server gets a failed scheme, not mainnet XRP booked or signed for with a testnet wallet.
- A server that reports more XRP for the account than the books can hold stops the scheme as failed.

## The wallet

The wallet is the original's, byte for byte, so the same passphrase finds the same account and its funds. It is derived (`keys.rs`) from the node's settlement entropy at `node/settlement-module/xrpl-testnet/seed-testnet` ([Identity](identity.md)) as an Ed25519 family seed. The 64 wallets of the identity chain's golden vectors pin the derivation. The node's deposit address is this account's.

## The Payment it signs

A settlement is a direct XRP `Payment` to the address the peer gave when peering, with one memo of the scheme's memo type naming our node ID: the only transaction the node signs (`codec.rs`).

- **Byte for byte as xrpl.js, which the original used, signs it.** Golden vectors (`crates/dassie-xrpl/tests/data/vectors.json`, made by `testdata/xrpl/generate-vectors.mjs`) pin the codec.
- **The settlement's ID is the transaction hash.** The message for the peer is the hash, so it can always be made again from the ID.
- **The memo type and what a `settlement` message means are a contract between peers.** Both sides of a peering must agree, so they change only with the peer protocol version.

## Following the account

The scheme books **every validated transaction that touches its account once, in ledger order**, from a cursor `(ledger, index)` kept in its state.

**Connecting.** On each connection: check the network and read the server's fees and reserves; subscribe to the account and the ledger stream; only then read the account at the validated ledger, so nothing falls between what it says and what the stream says next; catch up from the cursor with the account's history, holding the stream's transactions meanwhile; then make the books match the balance, book what was held, and submit what waits. A server whose history does not reach back leaves the balance to say what changed, through suspense, without who it was.

**Booking a validated transaction** by what it did to the account:

- **One of ours:** posted when it succeeded, voided when it failed, and its fee booked either way.
- **A settlement from a peer:** a successful Payment of XRP to us, whose memo has our memo type and names a peer, from the address that peer gave when peering. It is booked by `delivered_amount`, with its hash as the journal reference.
- **Anything else** that adds XRP, the account's creation included, is a deposit; anything that takes XRP, a withdrawal.

After each, the books must hold the balance less the reserve; any difference, such as a reserve change or a balance below the reserve, is reconciled through `equity/suspense`. **The reserve is never booked, so never spent.** A transaction of ours is settled whenever the ledger shows it, even behind the cursor: what the balance booked through suspense meanwhile comes back out.

**The first activation** books what the account holds above the reserve as the owner's deposit.

**The account gone** (a testnet reset): what the books hold is a loss through suspense, everything in flight is cancelled (an account made again starts its sequence anew), the cursor is dropped, and the faucet is asked until the account exists again.

## Settling

`prepare_settlement` is `Unavailable` ([Settlement](settlement.md#the-scheme-interface)) unless the scheme is synced and knows the account's sequence, has room for another transaction in flight, the amount is at least a drop, the network fee is below the scheme's ceiling (while it is higher, the scheme waits), and the amount and the fee fit above the reserve after what is in flight. Then it signs the Payment with the next sequence and keeps it, not yet executed. It settles whole drops.

- **Nothing is sent before it is booked.** A transaction is submitted only once the node executed its settlement, its pending transfer on disk. One never executed is never sent: past its last ledger it is dropped, and the sequence read again.
- **Cancelled only when it can never apply.** A malformed transaction (`tem…` on submit) is cancelled at once. Past its last ledger, a transaction is looked for over the ledgers it could have applied in; only when the server has searched all of them and it is not there is it cancelled. Anything less certain waits, since cancelling a transaction that applied would pay the peer twice.

## Settlement messages

The `settlement` message carries the transaction's hash, and books nothing: the ledger says what arrived, once. A Payment without the memo, from an address no peer gave, or that failed, is not a settlement.

## Deposits and withdrawals

The owner funds the scheme by sending XRP to its account. The node cannot deposit for the owner, and has no way to withdraw: a withdrawal is made outside the node, with the same keys, and booked when the ledger shows it.

## State and restarts

The scheme's state (cursor, next sequence, transactions in flight, peers' addresses, settlements being looked up) is kept with its checkpoint in the batch it explains ([Settlement](settlement.md#checkpoints-and-activation)), and a state the scheme could not have written is refused.

After a restart, executed transactions in flight are submitted again once synced. Each settlement the node still holds pending is handed to the scheme: one in flight is submitted again; one the scheme no longer holds is looked up by its hash and posted or voided by what the ledger says. A transaction of ours that no history holds any more cannot be known to have applied, and its settlement stays pending, for the owner.

## The fake ledger

The feature `fake` models the ledger for simulations and tests (`fake.rs`): accounts, reserves, fees, the checks the ledger makes on a Payment with their result codes, streams, history that can be cut short, network IDs, testnet resets and a faucet. It answers the subset of the rippled WebSocket API the scheme speaks, and checks what it is sent with the node's own codec. The node simulation settles on it in-process; `fake_ledger` serves it over WebSocket to the node's tests, the end-to-end tests and `node --run dev -- --xrpl fake`.

## Decisions

- [D8](../decisions/D08-real-money.md): the gates before real money, and the testnet-only network check.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): sans-IO, so the fake ledger can stand in for the network.
