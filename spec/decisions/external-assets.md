# External assets apart from settlement schemes

**Areas:** [Settlement](../areas/settlement.md#external-assets), [XRPL](../areas/xrpl.md#the-account), [Ledger](../areas/ledger.md), [Wallet](../areas/wallet.md#pages). Built, with the XRPL account as an external asset, RLUSD a second scheme on it ([XRPL](../areas/xrpl.md#rlusd)), and its holdings on a page of the wallet.

## Facts

- A settlement scheme that owns its account on the underlying ledger derives it, follows it, signs for it and books what it holds. That is one asset, one account, one scheme.
- A scheme that settles in a token on the XRP Ledger, such as RLUSD, pays its fees and its reserve in XRP, which its ledger's books (in dollars) have no place for.
- Two schemes that each signed for one XRPL account would take the same sequences: one of the two transactions fails, and the other scheme's books see a fee it did not pay as a withdrawal.
- The node decides from its own books ([own-books](own-books.md)): the owner's spending limit and the check before each settlement read the account that stands for what the node holds. If that account drifts from the ledger, the owner can spend money already gone, and the node owes its peers more than it can settle.
- More types will follow: other ledgers' accounts, Bitcoin outputs, bank accounts.

## Decision

Separate what the node holds from how it settles ([Settlement](../areas/settlement.md#external-assets)):

- **An external asset** is something the node holds outside its books, of a type; the node may hold none or several of each. It derives its keys, keeps the connection to its ledger, signs, hands out sequences, and reports its holdings as the ledger says.
- **A settlement scheme requires** external assets of certain types and holdings, and settles in one holding through them. Several schemes may share an external asset, which orders their transactions; each scheme keeps its own cursor into the asset's transactions.
- **The ledger is authoritative** for what an external asset holds: the wallet shows holdings from it. The books keep `assets/settlement` only to book against, and keep it in step with the holding the ledger's scheme settles in: every change booked, the rest reconciled through suspense.
- **A holding is booked only on the ledger whose scheme settles in it;** every change of it is booked there, whichever scheme caused it. A holding no scheme settles in is **gas, off the books**: the scheme settles nothing while the gas cannot pay its fee, and the owner sees the gas from the ledger.

## Accepted

- **Fees paid in gas leave no entry.** On a node that runs only an RLUSD scheme, the owner's revenue and expenses show no XRP fees; a report has to read them from the ledger. This holds for real money too: what decides is the settled holding, and that stays in step with the ledger. On an account another scheme settles from, the gas is that scheme's booked holding: it is spent only within the owner's funds on that ledger, and booked there.
- **A migration.** The XRPL scheme's state was split between the account and the scheme, once, with nothing booked anew. The versioned states that replaced it read none of it: the database drops what an account and its schemes kept before them, and the node reads the account anew from the ledger, a cost the testnet's reset makes small ([XRPL](../areas/xrpl.md#state-and-restarts)).

## Rejected

- *One account per scheme,* each derived from the scheme's name: no races, but two addresses for the owner to fund, two supplies of XRP for fees and reserves, and the same coupling of holding and scheme for every type to come.
- *Gas booked on the XRP ledger always:* the RLUSD scheme would need `xrpl-testnet+xrp` active, and its effects would book on two ledgers.
- *Fees converted into the scheme's currency at the current rate:* booking a fee would wait for a fresh rate, and a rate is not what the owner paid.
- *The books as the record of external holdings,* shown to the owner as they add up: the ledger already says what is there, and the books can only lag it.
