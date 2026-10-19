# XRPL

The `xrpl-testnet` settlement scheme: the node's wallet on the XRP Ledger testnet, derived from its passphrase; the Payments it signs to settle; how it follows its account and books every transaction that touches it once; how it survives restarts; and the gates that keep real money out.

**Paths:** `crates/dassie-xrpl/`, `crates/dassie-node/src/fake_ledger.rs`, `testdata/xrpl/`
**Read with:** [Settlement](settlement.md), [Ledger](ledger.md), [Identity](identity.md)

The crate does no IO and reads no clock. The scheme says what to send its ledger (`LedgerSend`) and books what the ledger says, through the node, over the connection the node keeps for it ([Settlement](settlement.md#schemes-that-follow-a-ledger)).

## Real money

Real money is refused at every level until the conditions of [D8](../decisions/D08-real-money.md) hold, which the [roadmap](../roadmap.md#real-money) lists. This scheme meets the XRP Ledger's part of them: the reserve is never booked (ACC-19), an account's creation is a deposit (ACC-20), it settles whole drops and books fees (ACC-21), and it checks results, books a failure's fee and checks the sender (ACC-22). Mainnet also needs a memo type of its own (ACC-13). Until then:

- The realm `live` is refused ([Node](node.md)).
- The ledger `xrpl+xrp` is in the live realm, and a scheme on a ledger outside the test realm does not activate ([Settlement](settlement.md#schemes-and-ledgers)).
- There is no mainnet scheme in the registry.
- **The testnet scheme refuses other networks.** Before it reads its account, it checks that its server reports the testnet's network ID, 1 (mainnet's is 0, devnet's 2). A server that reports another, or none, stops the scheme as failed ("the server is not on the XRP Ledger testnet; remove the scheme and add it with a testnet server"), before anything is booked, written off or signed. An owner who points it at a mainnet server gets a failed scheme, not mainnet XRP booked or signed for with a testnet wallet.
- A server that reports more XRP for the account than the books can hold (`MAX_TOTAL` units, about 9.2 × 10^9 XRP) stops the scheme as failed.

## Constants

| Name | Value |
|---|---|
| Scheme ID, ledger | `xrpl-testnet`, `xrpl-testnet+xrp` |
| Drops per XRP | 1,000,000 (the ledger counts in drops, scale 6) |
| Internal units per drop | 1000 (the books count XRP at scale 9) |
| Most XRP there is | 100 × 10^9 XRP |
| Memo type | `https://dassie.land/settlement/xrp-testnet/v1`, as UTF-8 bytes (hex on the wire) |
| Default server | `wss://s.altnet.rippletest.net:51233` |
| Default faucet | `https://faucet.altnet.rippletest.net/accounts` |
| Testnet network ID | 1 |
| Transactions in flight | at most 8 |
| Most fee | 1000 drops |
| Ledgers a transaction may take | 20 (`LastLedgerSequence` = last validated + 20) |
| Request timeout | 20 s, then the connection is made anew |
| Ledger silence | 30 s without a message, then the connection is made anew |
| Faucet retry | every 60 s while the account does not exist |
| Peers' addresses kept | 512, the one learnt longest ago goes first |
| Requests waiting for an answer | 2 · 8 + 8 + 4 = 28 |
| Transactions held while catching up | 256 |
| Settlements looked for after a restart | at most 64, 8 at a time |
| History page | 200 transactions |
| Config | at most 1024 bytes |

The memo type, and what a `settlement` message means, stay the same within one network version: both sides of a peering must agree, and they change only with the peer protocol version.

## The wallet

The wallet is the original's, byte for byte, so the same passphrase finds the same account and its funds:

1. `entropy = HMAC chain(node key, ["settlement-module", "xrpl-testnet", "seed-testnet"])`: the node's settlement entropy at `node/settlement-module/xrpl-testnet/seed-testnet` ([Identity](identity.md)).
2. `seed = entropy[0..16]`, an Ed25519 family seed; its text form is base58check with version bytes `0x01 0xE1 0x4B` (`sEd…`).
3. `private key = SHA-512(seed)[0..32]`; `public key = 0xED ‖ Ed25519 public key` (33 bytes).
4. `account ID = RIPEMD-160(SHA-256(public key))` (20 bytes); its address is base58check of `0x00 ‖ account ID` in the XRPL alphabet (`r…`). An address is 25 to 35 characters.

The 64 wallets of the identity chain's golden vectors pin the derivation, and the node's deposit address is this account's.

## Config

Empty for the testnet's server and faucet, or a JSON object with either of:

- `server`: a `wss://` URL, or `ws://`, which the node allows only where private addresses are allowed (dev and tests);
- `faucet`: an `https://` URL, or `null` to never ask.

Anything else, or more than 1024 bytes, is refused.

## Peering data

The scheme's payloads inside peer messages, in OER ([Peer protocol](peer-protocol.md)):

```
PeeringInfo     ::= SEQUENCE { address IA5String (SIZE(25..35)) }   -- a length byte, then the address
PeeringRequest  ::= PeeringInfo                                      -- the requester's address
PeeringResponse ::= empty (0 bytes)
SettlementProof ::= SEQUENCE { transactionHash OCTET STRING (SIZE(32)) }   -- 32 raw bytes
```

- The responder's peering info is its address; the requester learns it, and sends its own address as its request, which the responder learns. Each side keeps the other's address, in the same encoding, as the peer's scheme state.
- An address that does not parse refuses the request (reason 6, refused by the scheme).
- The scheme keeps the addresses of up to 512 peers in its own state too, to tell their settlements apart.
- It speaks scheme version 1.

## The Payment it signs

A settlement is a direct XRP `Payment`, the only transaction the node signs:

| Field | Value |
|---|---|
| `TransactionType` | `Payment` |
| `Account` | the node's account |
| `Destination` | the peer's address, from its scheme state |
| `Amount` | the settlement in whole drops: the internal amount divided by 1000, rounded down |
| `Fee` | from the server's state: `base fee × load factor / load base`, at least 10 drops |
| `Sequence` | the account's next sequence |
| `LastLedgerSequence` | the last validated ledger + 20 |
| `Flags` | 0 |
| `NetworkID` | left out: a transaction names its network only above 1024 |
| `Memos` | one memo: type the memo type above, data the node's ID as UTF-8 |
| `SigningPubKey`, `TxnSignature` | the wallet's public key, and its Ed25519 signature |

- Fields go in the ledger's canonical binary order, by type code and then field code. The signature covers `STX\0` and the fields without the signature; the transaction's hash is SHA-512Half of `TXN\0` and the signed blob.
- Payments are signed byte for byte as xrpl.js 3.1.0 signs them: golden vectors in `testdata/xrpl` pin the codec.
- The settlement's ID is the hash, in uppercase hex (64 characters). The message for the peer is the hash's 32 bytes, and can always be made again from the ID.

## Following the account

The scheme books **every validated transaction that touches its account once, in ledger order**, from a cursor `(ledger, index)` kept in its state.

**Connecting.** On each connection, in this order:
1. `server_state`: the network ID must be the testnet's ([above](#real-money)); the server's validated ledger, fees and reserves are read.
2. `subscribe` to the account and the `ledger` stream.
3. `account_info` at the validated ledger, once the subscription is on, so nothing falls between what it says and what the stream says next. A server behind the cursor is asked again when the next ledger closes. The next sequence is the account's, or one past the highest in flight.
4. **Catching up:** with a cursor behind the account's ledger, `account_tx` from the cursor's ledger to the account's, 200 a page, while the stream's transactions are held (at most 256). A server whose history does not reach back (`lgr…`) leaves the balance to say what changed, through suspense, without who it was. A busy server, or any other error, is tried again on a new connection.
5. **Synced:** the books are made to match the balance, the transactions held meanwhile are booked, and transactions waiting to be submitted are submitted.

The status is `Connecting` without a connection, `Syncing` while starting or catching up, `Unfunded` while the account does not exist, `Synced` when up to date, and `Failed(why)` once stopped. A request unanswered for 20 s, or a connection quiet for 30 s with no ledger closing, is made anew. So is one with as many requests waiting as it takes, or with more held transactions than fit.

**Booking a validated transaction.** Each changes the books by what it did to the account:

- **One of ours** in flight: posted (`tesSUCCESS`) or voided (anything else, `tec…`), and its fee booked either way (`Fee`, Dr `expenses/fees`).
- **A settlement from a peer:** a successful Payment of XRP to us, whose first memo has our memo type and names a peer as its data, from the address that peer gave when peering. It is booked as a settlement from it, by `delivered_amount`, with its hash as the journal reference.
- **Anything else that adds XRP**, the account's creation included: a deposit (ACC-20). **Anything that takes XRP:** a withdrawal of the whole difference.

After each, the books must hold the balance less the reserve (the base reserve plus the reserve per owned object); any difference, such as a reserve change or a balance below the reserve, is reconciled through `equity/suspense`. A reserve change seen as a ledger closes is reconciled at once. **The reserve is never booked, so never spent** (ACC-19).

A transaction of ours is settled whenever the ledger shows it, even behind the cursor: what the balance booked through suspense meanwhile comes back out.

**The first activation** books what the account holds above the reserve as the owner's deposit, and starts its cursor from `account_info`.

**The account gone** (a testnet reset): what the books hold is a loss through suspense, everything in flight is cancelled (an account made again starts its sequence anew), the cursor is dropped, and the faucet is asked: an HTTPS POST of `{"destination": "<address>"}`, again every 60 s until the account exists. When the faucet answers, the account is read again.

## Settling

`prepare_settlement` is `Unavailable` ([Settlement](settlement.md#the-scheme-interface)) unless all of these hold:

- the scheme is synced and not failed, and knows the account's sequence;
- fewer than 8 transactions are in flight;
- the amount is at least one drop;
- the fee is at most 1000 drops: while it is higher, the scheme waits;
- the amount and the fee fit in what is above the reserve, less what is in flight with its fees.

Then it signs the Payment, with consecutive sequences for the transactions in flight, and keeps it in flight, not yet executed. It settles whole drops: the amount it books is the drops times 1000.

- **Execution.** A transaction is submitted only once the node executed its settlement, its pending transfer on disk. One signed and then lost is cancelled. One never executed is never sent: past its last ledger it is dropped, and the sequence read again.
- **Submission.** Only `tem…` on submit cancels at once: the transaction is malformed and can never apply. `ter…` and `tel…` mark it to be submitted again. Any other answer waits: the transaction is submitted again after a reconnect or restart, and settled by the stream.
- **Past its last ledger,** it is looked for with `tx` over the ledgers from its submission to its last. Found validated, it is booked. Only when every one of those ledgers was searched and it is not there (`txnNotFound` with `searched_all`) is it cancelled, and the sequence read again. Otherwise it is asked again at the next ledger.

## Settlement messages

The `settlement` message carries the transaction's hash, 32 bytes; anything else is refused as invalid data. It books nothing: the ledger says what arrived, once. A Payment without the memo, from an address no peer gave, or that failed, is not a settlement.

## Deposits and withdrawals

The owner funds the scheme by sending XRP to its account (its deposit address) on the ledger. The node cannot deposit for the owner (`Unsupported`), and has no way to withdraw: a withdrawal is made outside the node, with the same keys, and booked when the ledger shows it.

## State and restarts

The scheme's state is JSON, kept with its checkpoint in the batch it explains ([Settlement](settlement.md#checkpoints-and-activation)):

```
{ "cursor":     [ledger, index] | null,
  "sequence":   next account sequence,
  "in_flight":  [{ "blob": hex, "drops", "fee", "sequence", "last_ledger", "submitted_at", "executed" }],
  "peers":      { "<node ID>": "<address>" },
  "recovering": [ hash in hex ] }
```

- A state with more than the scheme keeps (8 in flight, 512 peers, 64 recovering), a hash twice, or a transaction in flight at or above the next sequence is refused: the scheme did not write it.
- **After a restart** the state comes back with the checkpoint. Transactions in flight that were executed are submitted again once synced. Each settlement the node still holds pending is handed to the scheme: one in flight is marked executed and submitted again; one the scheme does not have in flight (its posting waited for an earlier announcement) is looked up by its hash with `tx`, at most 8 at a time, and posted or voided by what the ledger says.
- A transaction of ours that no history holds any more cannot be known to have applied, and its settlement stays pending, for the owner.

## The fake ledger

The feature `fake` models the ledger for simulations and tests: accounts with balances, sequences, owner counts and reserves (base 1 XRP, 0.2 XRP per object), the reference fee (10 drops), the checks the ledger makes on a Payment (signature, sequence, fee, last ledger, funds) with their result codes, metadata, streams and history (which can be cut short), ledgers that close on command, network IDs and testnet resets, and a faucet that gives 1000 XRP. It answers the subset of the rippled WebSocket API the scheme speaks, at API version 1, and checks what it is sent with the node's own codec.

- The node simulation settles on it in-process.
- `fake_ledger` serves it as rippled serves the real one: its WebSocket API on `ws://`, its faucet over HTTPS, and a ledger closed every so often. The node's tests, the end-to-end tests and `node --run dev -- --xrpl fake` settle against it.

## Decisions

- [D8](../decisions/D08-real-money.md): the gates before real money, and the testnet-only network check.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): sans-IO, so the fake ledger can stand in for the network.

## Original

- [5 §4.5](../original/05-accounting-and-settlement.md): the seed-path chain for settlement modules.
- [5 §6.2–§6.6](../original/05-accounting-and-settlement.md): the original's XRPL modules, constants, wallet derivation, OER payloads, and the stream handler that booked by balance changes, with the gaps ACC-13 and ACC-19 to ACC-22.
