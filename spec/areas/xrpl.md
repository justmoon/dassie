# XRPL

The node's account on the XRP Ledger testnet and the `xrpl-testnet` settlement scheme on it: the wallet, derived from the owner's passphrase; the account as an external asset, which connects, signs and follows the ledger for the schemes on it; the Payments the scheme signs to settle; how every transaction that touches the account is booked once; how both survive restarts; and the gates that keep real money out.

**Paths:** `crates/dassie-xrpl/`, `crates/dassie-node/src/fake_ledger.rs`, `testdata/xrpl/`
**Read with:** [Settlement](settlement.md), [Ledger](ledger.md), [Identity](identity.md)

The crate does no IO and reads no clock. The account says what to send its ledger and the node feeds it what comes back, over the one connection it keeps for the account; the account tells the schemes on it what the ledger said, and they book it, through the node ([Settlement](settlement.md#external-assets)). Its constants are in `lib.rs`, `account.rs` and `scheme.rs`.

The account is an external asset (`account.rs`), and `xrpl-testnet` is a scheme on it that settles in its XRP (`scheme.rs`). The crate reads and writes tokens too ([Tokens](#tokens)), but nothing signs or books one yet: RLUSD, as a second scheme on the same account, is [not built](../roadmap.md#external-assets-and-rlusd).

## Real money

Real money is refused at every level until the conditions of [real-money](../decisions/real-money.md) hold, which the [roadmap](../roadmap.md#real-money) lists. The scheme meets the XRP Ledger's part of them (the reserve is never booked, an account's creation is a deposit, it settles whole drops, books fees and checks results and senders); mainnet also needs a memo type of its own. Until then:

- The realm `live` is refused ([Node](node.md)), a scheme on a ledger outside the test realm does not activate ([Settlement](settlement.md#schemes-and-ledgers)), and there is no mainnet scheme.
- **The testnet account refuses other networks.** Before it reads the account, it checks that its server reports the testnet's network ID. A server that reports another, or none, stops the account as failed, and every scheme on it with it, before anything is booked, written off or signed. An owner who points it at a mainnet server gets a failed scheme, not mainnet XRP booked or signed for with a testnet wallet.
- A server that reports more XRP for the account than the books can hold stops the account as failed.
- **Only the testnet resets** ([ledger-resets](../decisions/ledger-resets.md)). Taking a ledger below the newest seen for a new chain, and writing off an account gone with it, are gated on the network ID by a check of their own (`resets` in `account.rs`), not only by the one that keeps the account on the testnet. On any other network, a lower ledger stops the account, and so does the absence of an account that a scheme on it held anything of, with nothing written off. Letting the account run on another network can never make a lower ledger acceptable.

## The wallet

The wallet is the original's, byte for byte, so the same passphrase finds the same account and its funds. It is derived (`keys.rs`) from the node's settlement entropy at `node/settlement-module/xrpl-testnet/seed-testnet` ([Identity](identity.md)) as an Ed25519 family seed. The 64 wallets of the identity chain's golden vectors pin the derivation. That path belongs to the node's first XRPL testnet account (`FIRST` in `account.rs`), whichever schemes settle through it; further accounts get paths of their own when they are built, and no path gets a new meaning. The node's deposit address is this account's.

## The account

The account owns what the schemes that settle through it would otherwise race for ([external-assets](../decisions/external-assets.md)):

- **The connection:** the network check, the newest ledger seen, testnet resets and the faucet ([below](#following-the-account)). The server and the faucet are the account's config, which the scheme that activates it names; a scheme that names others while the account is active is refused.
- **What it signs:** the next sequence and the transactions in flight. A scheme asks the account to sign; the account hands out sequences in order, so two schemes on one account never take the same one, and keeps each transaction with the scheme that signed it. It signs nothing unless it is synced and knows its sequence, has room for another transaction in flight, the amount is at least a drop, the network fee is at most its ceiling (while it is higher, it waits), and the amount and the fee fit above the reserve after what is in flight.
- **What it holds,** as the ledger last said: its XRP and the reserve, which the node shows from the account, never from the books ([Settlement](settlement.md#external-assets)).
- **What it learnt,** which it tells every scheme on it, in order: each validated transaction that touches the account, with the scheme that signed it if it was in flight; that it is synced; that a transaction can never apply; that the account is gone; each ledger that closes; and each transaction a scheme looked up.

**Each scheme keeps its own cursor** into the account's transactions, and tells the account where it is. On each connection the account reads its history from the earliest cursor of its schemes, and each scheme books only what is past its own, so a scheme that stopped reads again what it missed, and the others are not held back.

## The Payment it signs

A settlement is a direct XRP `Payment` to the address the peer gave when peering, with one memo of the scheme's memo type naming our node ID: the only transaction the account signs (`codec.rs`).

- **Byte for byte as xrpl.js, which the original used, signs it.** Golden vectors (`crates/dassie-xrpl/tests/data/vectors.json`, made by `testdata/xrpl/generate-vectors.mjs`) pin the codec.
- **The settlement's ID is the transaction hash.** The message for the peer is the hash, so it can always be made again from the ID.
- **The memo type and what a `settlement` message means are a contract between peers.** Both sides of a peering must agree, so they change only with the peer protocol version.

## Tokens

The codec and the API's parser also cover tokens on trust lines, such as RLUSD (`amount.rs`, `codec.rs`, `api.rs`), for the RLUSD scheme to come ([designs/external-assets.md](../designs/external-assets.md#4-rlusd-on-the-xrpl-testnet)).

- **Amounts are exact.** A token's value is decimal, a mantissa of sixteen digits and an exponent, held in the ledger's one canonical form and never as a float. A value the ledger cannot express exactly, in text or in bytes, is refused rather than rounded, so what the node reads is what the ledger holds.
- **Whole units convert exactly.** A value becomes the whole units of a ledger rounded down, toward minus infinity, so the books never hold more than the line. Units become a value only when the ledger can express it exactly (sixteen significant digits), so the node never signs an amount other than the one it booked.
- **The transactions** are a token `Payment`, without paths or `SendMax`, and a `TrustSet`, which sets a line's limit and flags. A Payment is never partial: the encoder asserts it and the decoder refuses it, so a payment delivers its whole amount or fails. Golden vectors from xrpl.js pin both, and token amounts at the edges (the least and the largest value, sixteen digits, zero), byte for byte.
- **A line reads the same from either side.** The ledger keeps a line's balance from the side of the account whose ID is the lower number, and its flags in a low and a high half. The parser gives the balance, the limits and the flags (NoRipple, freeze and deep freeze, as each side set them) from the side asked for, so no caller depends on which number is lower; a line whose limits name its accounts out of order is refused.
- **What a transaction did to a line,** from its metadata: the line before and after, created from zero or deleted to zero. A settlement, a deposit, a withdrawal and an issuer's clawback, which is no Payment, all show as a change of the line's balance. A Payment's delivered amount is read when it is a token.
- **Reading the ledger:** the line itself (`ledger_entry`), and the issuer's account (`account_info`): its global freeze and its transfer rate. Each answer names the ledger it is as of, so a scheme can take only what is [newer](#following-the-account).

## Following the account

The scheme books **every validated transaction that touches its account once, in ledger order**, from its cursor `(ledger, index)`, kept in its state.

**Only what is newer** ([ledger-resets](../decisions/ledger-resets.md)). The account acts only on what a server says as of a validated ledger at or past the newest it has seen, by the ledger stream, a server's state or what a scheme on it booked. A public address spreads its connections over several servers, and one may answer from a ledger minutes old: its answers, an absent account included, are not acted on, and are asked for again as ledgers close, until it has caught up. Validated ledgers are final, so a server behind can only delay the account. The newest ledger seen, with its close time, is kept with the account's state.

**The server is trusted.** The account takes its server's word on which ledgers are validated and what they hold: it checks no validator's signature. Only what is newer protects against a server that lags, not one that lies, which can have a scheme book anything, or claim a testnet reset. Checking the validators against a UNL of the node's own is [not built](../roadmap.md#checking-xrp-ledger-validators).

**Connecting.** On each connection: check the network, read the server's fees and reserves, and tell from its validated ledger whether it is behind or on a chain started again; subscribe to the account and the ledger stream; only then read the account at the validated ledger, so nothing falls between what it says and what the stream says next; catch up from the earliest cursor with the account's history, holding the stream's transactions meanwhile; then the schemes make their books match the balance and book what was held, and the account submits what waits. A server whose history does not reach back leaves the balance to say what changed, through suspense, without who it was.

**Booking a validated transaction** by what it did to the account:

- **One of ours:** posted when it succeeded, voided when it failed, and its fee booked either way.
- **A settlement from a peer:** a successful Payment of XRP to us, whose memo has our memo type and names a peer, from the address that peer gave when peering. It is booked by `delivered_amount`, with its hash as the journal reference.
- **Anything else** that adds XRP, the account's creation included, is a deposit; anything that takes XRP, a withdrawal.

**Peers' addresses** are the scheme's. They are learnt only once a peering is made, never from an attempt that may still be refused, and kept after it ends, like the [settlement channel](settlement.md#settlement-channels) the peer's settlements come over. The scheme keeps at least as many as the node has channels (asserted at start), so no stranger's attempt displaces a peer's address and turns its settlement into the owner's deposit.

After each, the books must hold the balance less the reserve; any difference, such as a reserve change or a balance below the reserve, is reconciled through `equity/suspense`. **The reserve is never booked, so never spent.** A transaction of ours is settled whenever the ledger shows it, even behind the cursor: what the balance booked through suspense meanwhile comes back out.

**The first sync** books what the account holds above the reserve as the owner's deposit. Only then does the node peer on the scheme, so no peer's settlement is taken for that deposit ([Settlement](settlement.md#the-scheme-interface), [synced-before-peering](../decisions/synced-before-peering.md)). The cursor it sets is the record of that sync, kept with the scheme's state.

**The account gone** (a testnet reset). A reset starts the chain again from a first ledger far below the newest seen, which the rule above alone would never act on. A server whose validated ledger is below the newest seen but closed after it is on a new chain: close times only grow along a chain, so a server behind cannot report that. On the testnet only, the account then follows the new chain. Then, or when the account is absent as of a ledger it has not seen past: each scheme books what its books hold as a loss through suspense and drops its cursor, everything in flight is cancelled (an account made again starts its sequence anew), and the faucet is asked until the account exists again. The node takes no new peering on the scheme until it has synced again.

## Settling

`prepare_settlement` is `Unavailable` ([Settlement](settlement.md#the-scheme-interface)) unless the account signs the Payment now ([above](#the-account)). It settles whole drops, and the transaction it signs is kept in flight, not yet executed.

- **Nothing is sent before it is booked.** A transaction is submitted only once the node executed its settlement, its pending transfer on disk. One never executed is never sent: past its last ledger it is dropped, the sequence read again, and the account remembers it as never sent until its scheme has reconciled it ([below](#state-and-restarts)). Its sequence goes to the next transaction, whichever scheme asked.
- **Cancelled only when it can never apply.** A malformed transaction (`tem…` on submit) is cancelled at once. Past its last ledger, a transaction is looked for over the ledgers it could have applied in; only when the server has searched all of them and it is not there is it cancelled. Anything less certain waits, since cancelling a transaction that applied would pay the peer twice.

## Settlement messages

The `settlement` message carries the transaction's hash, and books nothing: the ledger says what arrived, once. A Payment without the memo, from an address no peer gave, or that failed, is not a settlement.

## Deposits and withdrawals

The owner funds the scheme by sending XRP to its account. The node cannot deposit for the owner, and has no way to withdraw: a withdrawal is made outside the node, with the same keys, and booked when the ledger shows it.

The account takes payments with or without a destination tag: it is the node's alone, so tags tell nothing apart and are ignored.

**The owner may ask the faucet** of a test ledger for more test XRP (`AskFaucet`), as the account asks while it does not exist. It is the same request, at the same pace: at most one at a time and one a minute, so the owner adds no load the faucet would not see anyway. It is asked only on a connection, since its answer is taken on one. Nothing is booked at the asking: what the faucet sends is a deposit once the ledger shows it, by the rule above. A ledger without a faucet (the config's `faucet` is `null`, as on the live ledger) refuses.

## State and restarts

Each keeps its own state, written with its checkpoint in the batch it explains ([Settlement](settlement.md#checkpoints-and-activation)): the account its next sequence, its transactions in flight (each with the scheme that signed it), those it remembers as never sent, and the newest ledger seen; the scheme its cursor, its peers' addresses and the settlements it looks up. A state either could not have written is refused, and so is a scheme state that still holds the account's: the database migrates one written before the account kept its own into the two (`store/schema.rs`), and a node that restarts on it books nothing anew.

After a restart, executed transactions in flight are submitted again once synced. Each settlement the node still holds pending is handed to its scheme:

- **In flight:** submitted again.
- **Never sent:** cancelled at once. It was signed and booked but never executed, and dropped past its last ledger, so no ledger will ever hold it; a scheme that stopped and starts again cancels it rather than leave it pending for good.
- **Neither:** looked up by its hash and posted or voided by what the ledger says. It is looked for in all the server's history, without a range of ledgers: when it was submitted is not kept, and a server refuses to search more than a short range at once. A transaction of ours that no history holds any more cannot be known to have applied, and its settlement stays pending, for the owner; it is asked for again less often the longer it is not found, so a lookup that never succeeds costs little.

## The fake ledger

The feature `fake` models the ledger for simulations and tests (`fake.rs`): accounts, reserves, fees, the checks the ledger makes on a Payment with their result codes, close times, streams, history that can be cut short, network IDs, testnet resets that start the chain again from a lower ledger, a faucet, and trust lines with an issuer of test RLUSD ([below](#tokens-in-the-fake-ledger)). A copy of it is a server behind the others, stopped where the ledger was. In-process its close times are fixed steps, so a simulation stays deterministic; served by `fake_ledger`, its ledgers close on the wall clock, so a new server is a new chain that closes later, and nodes that knew an earlier one see a testnet reset. It answers the subset of the rippled WebSocket API the account speaks, and checks what it is sent with the node's own codec. The node simulation settles on it in-process; `fake_ledger` serves it over WebSocket to the node's tests, the end-to-end tests and `node --run dev -- --xrpl fake`.

### Tokens in the fake ledger

The fake ledger is the only source of test RLUSD for dev, the end-to-end tests and the simulations: Ripple hands out testnet RLUSD only by hand. So it models what an RLUSD scheme meets on the testnet, and what it cannot get from the real one on demand: freezes and clawbacks (`fake/tokens.rs`).

- **Its issuer is RLUSD's,** at the testnet issuer's address, with its settings: lines ripple through it, it requires no authorisation, and it keeps the right to freeze and to claw back. A scheme with RLUSD's constants runs on the fake unchanged. The issuer outlives a reset, as one that came back at its address; its lines and what they held do not. A test can add issuers, so a line is seen from both sides, whichever account's ID is the lower number.
- **Trust lines** are kept as the ledger keeps them ([Tokens](#tokens)): the balance from the low side, each side's limit and flags, and an owner reserve on each side not in its default state, normally the holder's. A `TrustSet` sets the sender's limit and flags, with the ledger's refusals, such as a deep freeze on a line not frozen, or a line the account lacks the reserve for (its first two are free). A line back in its default state on both sides is deleted, and its reserve freed.
- **A token Payment** goes over the line between the two when either issues the token, else through the issuer, and fails as the ledger fails it. Nothing to pay with, no line, a payer frozen, a payee deep-frozen, or a global freeze between holders: `tecPATH_DRY`. Less than all of it, above the payee's limit, or an issuer's transfer fee, which a Payment without `SendMax` cannot pay: `tecPATH_PARTIAL`. No payee: `tecNO_DST`. A frozen line still receives, and still pays its issuer. A failure costs its fee and moves no token.
- **The issuer acts on request,** without a key: it mints (a Payment to a holder), freezes and deep-freezes a line (a `TrustSet` of its own), freezes every line or sets a transfer fee (`AccountSet`), and claws back (`Clawback`, which lowers the line with no Payment). Each applies in the next ledger as any transaction does, with the metadata, delivered amount, history and streams the ledger gives: both accounts of a line hear what changes it, and an `AccountSet` reaches only the issuer's own stream. `ledger_entry` reads a line, and `account_info` the issuer's flags and transfer rate. `fake_ledger` takes these requests over HTTPS next to the faucet, under `/tokens/`, for dev and the end-to-end tests.
- **Its arithmetic is exact.** Where the ledger would round a balance to sixteen digits, the fake refuses the transaction (`tecPRECISION_LOSS`) instead of guessing how the ledger rounds; a scheme books by what the line holds, which covers the ledger's rounding too. As the XRP that exists never changes, every token's holders hold exactly what its issuer minted, less what came back to it, was clawed back or wiped, after every ledger.

## Decisions

- [external-assets](../decisions/external-assets.md): the account apart from the schemes that settle through it.
- [real-money](../decisions/real-money.md): the gates before real money, and the testnet-only network check.
- [tigerstyle](../decisions/tigerstyle.md): sans-IO, so the fake ledger can stand in for the network.
- [synced-before-peering](../decisions/synced-before-peering.md): no peering on the scheme before its first sync.
- [ledger-resets](../decisions/ledger-resets.md): only what is newer than the newest ledger seen; a reset is a lower ledger that closed later, and only on the testnet.
