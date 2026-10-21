# A limit no payment can use is caught before approval

**Areas:** [Payments](../areas/payments.md), [Wallet](../areas/wallet.md).

## Facts

- An outgoing payment debits its wallet address's asset, and a debit limit caps that amount. A limit in another asset, by code or by scale, fits no payment from that wallet, so every payment made under the grant is refused.
- The Open Payments specs leave it to the authorization server whether it checks a limit's asset.
- The node answers grant requests and payments itself, and knows the asset of each of its wallet addresses when a request names one. It does not need to ask anyone.
- A receive limit caps what the receiver gets, in the receiver's asset. A grant that names no receiver may pay any receiver, so the asset of its receive limit only narrows which receivers it can pay. One that names a receiver can pay only that one, and the receiver's asset is known only by reading it from its node, which may be another node.
- The likely mistake is a matching code at another scale, such as USD in cents for a wallet that counts in billionths. The consent page then shows nothing wrong.

## Decision

The node refuses what it knows locally, and flags what it can learn only from another node.

- **A debit limit in another asset** than the item's wallet address, on an item that creates outgoing payments, gets the request refused with a 400. The answer names the asset the limit must be in. The check is made where the node resolves the item's wallet address, before the owner is asked.
- **A receive limit in another asset** than the receiver the item names is flagged on the consent page. The node compares it with the receiver it reads for the page, by the same rule a payment is checked by. The owner may still approve.
- **A payment refused for a limit's asset** names the limit and both assets, so a client that holds such a grant learns why.

## Accepted

- A request the specs allow is refused. A client that sent it would get a grant no payment could use anyway.
- A debit limit on an item that only reads or lists is not checked. It caps no payment there.
- An owner who approves a flagged grant gets one that cannot pay its receiver. The flag informs; the owner decides.
- A receiver that cannot be read is not judged, so its grant is not flagged.
- A receive limit on an item that names no receiver is not flagged. It narrows which receivers the app can pay, which is what the limit is for.

## Rejected

- *Check a limit's asset only when a payment is made:* the owner approves a grant that can never pay, the app learns why only when it pays, and the owner never learns.
- *Refuse a receive limit in another asset than the receiver the grant names:* checking it means reading the receiver from its node while answering the request, which can be slow or fail.
- *Disable Approve on a flagged grant:* the wallet shows what the node reports and holds no rule of its own ([wallet.md](../areas/wallet.md)), and the owner may have reasons the node cannot see.
- *Tell the client at grant time:* GNAP's answers have no place for a warning, and the receiver is read only once the owner is asked.
