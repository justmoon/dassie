# A hub may hold an issued token; its issuer's freeze pauses it, a clawback ends it, and its fees are paid in XRP anyone may send

**Areas:** [Settlement hubs](../designs/settlement-hubs.md#6-deposits-and-withdrawals), [XRPL](../areas/xrpl.md#tokens), [Settlement](../areas/settlement.md). Not built yet ([roadmap](../roadmap.md#token-hubs)).

## Facts

- Peers settle only over hubs ([hubs-only](hubs-only.md)), so a dollar token needs a hub of its own: DSUSD on the XRP Ledger testnet, RLUSD on mainnet ([mainnet-schemes](mainnet-schemes.md), [dsusd](dsusd.md)).
- A token on the XRP Ledger is held on a trust line to its issuer, which can freeze a holder's line, or every line at once with a global freeze, and claw back a holder's balance ([issued-assets](issued-assets.md)). A frozen line pays no one but the issuer. A hub's escrow is such a holder, and its line holds every member's funds.
- The issuer's acts are proven like any other state or transaction, from a ledger the network's UNL validated ([verified-ledgers](verified-ledgers.md)), so validators can attest them as they attest deposits.
- A hub that stalls pays out with no notice period, and one that shuts down never runs again ([the design](../designs/settlement-hubs.md#9-shutdown)). A freeze can be lifted.
- A payee that took a Prepare with its commitment has forwarded the packet, and pays its downstream once the Fulfill comes back; the claim on the hub is how it is paid for it ([the design](../designs/settlement-hubs.md#4-peering-over-a-hub)).
- Every transaction on the XRP Ledger pays its fee in XRP, from the account that sends it. A token escrow's own fees, for withdrawals, signer lists and payout sets, so come from its XRP, while the hub's own fees are in the token. The XRP Ledger lets an account pay fees from XRP its reserve locks.
- Anyone can send XRP to any account, and can add objects to the escrow's owner directory, which change the reserve an account needs.
- Whether a dollar token is worth a dollar is a price, which no ledger proves to the validators; the nodes already watch it ([issued-assets](issued-assets.md)).

## Decision

- **A hub may hold an issued token,** named in its rules with its issuer. The escrow holds a trust line to the issuer, set at founding with rippling disabled, and the validators check it with the rest of the escrow's setup before the first block. The first token hubs are DSUSD on the testnet and RLUSD on mainnet.
- **A clawback from the escrow ends the hub.** Once q validators attested it, the hub shuts down, and the payout set pays every account its share of what the escrow still holds, pro rata to what the hub owed it, with a rule for the remainder that pays out no more than the escrow holds.
- **A freeze is a state of the hub's ledger, never a stall.** A freeze of the escrow's line, or the issuer's global freeze, once q validators attested it, puts the hub in a frozen state from that block. While frozen, the validators take no operation but claims for commitments signed before the freeze's block, and those only until they expire; nothing is submitted to the escrow. A thaw, attested the same way, resumes the hub where it stood.
- **XRP sent to a token escrow is gas, never a deposit.** Anyone may send it; it credits no account and is never paid out as a balance. The escrow's XRP pays the XRP Ledger's fees for the hub's transactions.
- **A member tops up the gas before it asks for a withdrawal.** It reads the escrow's absolute XRP balance, and if that is below a minimum fixed in the code, about 1 XRP, it first sends the escrow enough XRP itself. The minimum is absolute: no reserve is counted from the escrow's objects, since the XRP Ledger lets the reserve pay fees.
- **A de-peg is the nodes' business.** The hub moves the token as it is; each node decides what it converts at and whether it converts at all ([issued-assets](issued-assets.md)). Nothing in the hub reacts to a de-peg, and the first release with real money adds nothing for it.

## Accepted

- **A frozen hub holds its members' funds** until the issuer thaws it, for as long as that takes, and a clawback costs every member its share.
- **Payees keep only claims for packets in flight.** What a payee had not yet taken from its holds by claim or redeem before the freeze's block waits for the thaw.
- **A member that withdraws pays the gas for everyone,** a small cost on each withdrawal, and XRP sent to the escrow stays there.
- **The issuer is every member's counterparty** for the whole of what the hub holds, as it is for any holder of its token ([issued-assets](issued-assets.md)).

## Rejected

- *A frozen hub that keeps running:* its balances may be worth nothing once the issuer acts again, and nodes that see that sooner would move their value out of the hub through others, leaving it with those who did not.
- *A freeze as a stall:* a stall pays out with no notice and cannot resume, for a freeze the issuer may lift, and a payout from a frozen line fails anyway.
- *Stopping claims for commitments signed before the freeze:* payees that forwarded those packets, and pay their downstream for them, would lose them.
- *Validators with exchange rates,* to price the token or react to a de-peg: a rate no ledger proves, agreed in consensus, when each node already prices the token itself.
- *A float of XRP the sponsor funds:* the sponsor has no part in a running hub, and the float would run dry with no one bound to refill it.
- *A separate XRP pool for fees:* another account to found, check and pay out, with keys of its own, for what the escrow's own XRP does.
- *A minimum counted from the escrow's reserve:* whoever adds objects to the escrow's owner directory would change what members must pay in, and XRP above a counted reserve would look like a balance someone could claim.
