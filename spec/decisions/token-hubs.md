# A hub may hold an issued token; its issuer's freeze pauses it, a clawback ends it, and its fees are paid in XRP anyone may send

**Areas:** [Settlement hubs](../designs/settlement-hubs.md#6-deposits-and-withdrawals), [XRPL](../areas/xrpl.md#tokens), [Settlement](../areas/settlement.md). Not built yet ([roadmap](../roadmap.md#token-hubs)).

## Facts

- Peers settle only over hubs ([hubs-only](hubs-only.md)), so a dollar token needs a hub of its own: DSUSD on the XRP Ledger testnet, RLUSD on mainnet ([mainnet-schemes](mainnet-schemes.md), [dsusd](dsusd.md)).
- A token on the XRP Ledger is held on a trust line to its issuer, which can freeze a holder's line, deep-freeze it, or freeze every line at once with a global freeze, and claw back a holder's balance ([issued-assets](issued-assets.md)). A frozen line pays no one but the issuer, though a line frozen alone, without a deep freeze, can still be paid. A hub's escrow is such a holder, and its line holds every member's funds.
- An issuer may charge a fee on transfers between holders, which it can set or change at any time. A Payment from a holder then delivers the amount only if the sender pays more, up to a `SendMax`, and fails without one ([XRPL](../areas/xrpl.md#rlusd)).
- The issuer's acts are proven like any other state or transaction, from a ledger the network's UNL validated ([verified-ledgers](verified-ledgers.md)), so validators can attest them as they attest deposits.
- A hub that stalls pays out with no notice period, and one that shuts down never runs again ([the design](../designs/settlement-hubs.md#9-shutdown)). A freeze can be lifted.
- A payee that took a Prepare with its commitment has forwarded the packet, and pays its downstream once the Fulfill comes back; the claim on the hub is how it is paid for it ([the design](../designs/settlement-hubs.md#4-peering-over-a-hub)).
- A commitment carries no time it was signed, and the payer signs it off the hub. What a validator can check is its expiry, which is the Prepare's less the hub's margin M, and a Prepare lives at most 30 seconds ([connector-timing](connector-timing.md)).
- A withdrawal's Payment and a payout set's Payments are public once q validators signed them, and have no last ledger, so anyone, the issuer included, can submit them whenever they like. One that fails uses its sequence ([the design](../designs/settlement-hubs.md#6-deposits-and-withdrawals)).
- Every transaction on the XRP Ledger pays its fee in XRP, from the account that sends it. A token escrow's own fees, for withdrawals, signer lists and payout sets, so come from its XRP, while the hub's own fees are in the token. The XRP Ledger lets an account pay fees from XRP its reserve locks.
- Anyone can send XRP to any account. Only the escrow's own objects raise its reserve; objects others create that name it are charged to their creator ([the design](../designs/settlement-hubs.md#2-founding-a-hub)). The reserve each object costs is the network's, which its validators vote to change.
- Whether a dollar token is worth a dollar is a price, which no ledger proves to the validators; the nodes already watch it ([issued-assets](issued-assets.md)).

## Decision

- **A hub may hold an issued token,** named in its rules with its issuer. The escrow holds a trust line to the issuer, set at founding with rippling disabled, and the validators check it with the rest of the escrow's setup before the first block. The first token hubs are DSUSD on the testnet and RLUSD on mainnet.
- **No transfer fee.** A hub takes only a token whose issuer charges none: the validators' check before the first block reads the issuer's account and refuses a transfer rate. A fee the issuer sets later freezes the hub, as a freeze does, until the issuer removes it.
- **A clawback from the escrow ends the hub.** Once q validators attested it, the hub shuts down. Withdrawals the escrow already sequenced count as paid in full: the payout set keeps them first, as already signed. What the escrow still holds beyond them is paid to every account pro rata to what the hub owed it, with a rule for the remainder that pays out no more than the escrow holds.
- **A freeze is a state of the hub's ledger, never a stall.** A freeze or deep freeze of the escrow's line, or the issuer's global freeze, once q validators attested it, puts the hub in a frozen state from that block. While frozen, the validators take no operation but `claim`, and only for commitments whose expiry is no later than the freeze block's stamp plus the longest a Prepare lives (30 s), each before its own expiry as always. That covers every packet a payee took up to M after the freeze's stamp, by when the block is normally final. Every other operation, deposits and the outcomes of withdrawals included, waits. A thaw, attested the same way, resumes the hub where it stood.
- **A frozen hub's method is unavailable,** as a failed hub's is ([hubs-only](hubs-only.md)): from the freeze's block on, members send no Prepare over it, take none, sign no new commitment and deposit nothing, and a peering keeps its methods over other hubs. Once the thaw is final, the method is available again.
- **Nothing is submitted to a frozen escrow.** Validators and members submit nothing while the escrow's line or the issuer is frozen, or the issuer charges a transfer fee, as of the latest ledger they proved, and a validator signs no payout set until the thaw: a stall or shutdown during a freeze pays out after it. A deposit that lands during a freeze is credited once the hub resumes, and a withdrawal someone submits anyway fails and is returned, less its fee, as any failed withdrawal.
- **XRP sent to a token escrow is gas, never a deposit.** Anyone may send it; it credits no account and is never paid out as a balance. The escrow's XRP pays the XRP Ledger's fees for the hub's transactions.
- **A member tops up the gas before it asks for a withdrawal.** It reads the escrow's absolute XRP balance, and if that is below a minimum fixed in the code, about 1 XRP, it first sends the escrow enough XRP itself. The minimum is absolute: no reserve is counted from the escrow's objects, since the XRP Ledger lets the reserve pay fees.
- **A de-peg is the nodes' business.** The hub moves the token as it is; each node decides what it converts at and whether it converts at all ([issued-assets](issued-assets.md)). Nothing in the hub reacts to a de-peg, and the first release with real money adds nothing for it.

## Accepted

- **A frozen hub holds its members' funds** until the issuer thaws it, for as long as that takes, and a clawback costs each member its share of it, but for withdrawals already sequenced.
- **Payees keep only claims for packets in flight.** What a payee had not yet taken from its holds by claim or redeem before the freeze's block waits for the thaw.
- **A window after the freeze.** For up to 30 seconds after the freeze's stamp, commitments signed after it can still be claimed, since no validator can tell them from earlier ones. Only a payee that takes a Prepare over a hub it knows is frozen lets value move that way, and no more than the payer's holds toward it.
- **A member that learns of the freeze late,** more than M after its stamp, as one far from the validators may, loses the packets it took in between, as it books any it learned of too late.
- **A set signed before a freeze can still be failed.** A payout set that reached q signatures before the escrow froze is public, so anyone can submit its Payments during the freeze; each fails and uses its sequence, and what it would have paid stays in the escrow for good. Members submit a set as soon as it has q signatures, which keeps that window short.
- **A pending withdrawal paid in full ahead of a clawback:** its Payment is signed and public, so anyone can submit it whatever the hub decides. If the escrow holds less than such withdrawals need, the ones that fail leave their amounts in the escrow.
- **A member that withdraws pays the gas for everyone,** a small cost on each withdrawal, and XRP sent to the escrow stays there.
- **The issuer is every member's counterparty** for the whole of what the hub holds, as it is for any holder of its token ([issued-assets](issued-assets.md)).

## Rejected

- *A frozen hub that keeps running:* its balances may be worth nothing once the issuer acts again, and nodes that see that sooner would move their value out of the hub through others, leaving it with those who did not.
- *A freeze as a stall:* a stall pays out with no notice and cannot resume, for a freeze the issuer may lift, and a payout from a frozen line fails anyway.
- *Stopping claims at the freeze's block:* payees that forwarded packets just before it, and pay their downstream for them, would lose them.
- *Claims only for commitments signed before the freeze:* a commitment carries no time it was signed, so no validator could check it.
- *Paying with `SendMax` under a transfer fee, and booking the fee per payout:* the escrow would pay out more than each balance it owes, at a rate the issuer can change at any time, so either members bear a fee no rule bounds or the escrow runs short.
- *Validators with exchange rates,* to price the token or react to a de-peg: a rate no ledger proves, agreed in consensus, when each node already prices the token itself.
- *A float of XRP the sponsor funds:* the sponsor has no part in a running hub, and the float would run dry with no one bound to refill it.
- *A separate XRP pool for fees:* another account to found, check and pay out, with keys of its own, for what the escrow's own XRP does.
- *A minimum counted from the escrow's reserve:* every member would have to read the reserve from proven state and the network's reserve settings, which change by vote, and agree on it, and XRP above a counted reserve would look like a balance someone could claim.
