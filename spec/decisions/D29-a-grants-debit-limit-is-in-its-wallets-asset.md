# D29. A grant's debit limit is in its wallet's asset

**Areas:** [Payments](../areas/payments.md).

## Facts

- An outgoing payment debits its wallet address's asset, and a debit limit caps that amount. A limit in another asset, by code or by scale, fits no payment from that wallet, so every payment made under the grant is refused.
- The Open Payments specs leave it to the authorization server whether it checks a limit's asset.
- The node answers grant requests and payments itself, and knows the asset of each of its wallet addresses when a request names one. It does not need to ask anyone.
- A receive limit caps what the receiver gets, in the receiver's asset. A grant that names no receiver may pay any receiver, so the asset of its receive limit only narrows which receivers it can pay. One that names a receiver can pay only that one, and the receiver's asset is known only by reading it from its node, which may be another node.
- The likely mistake is a matching code at another scale, such as USD in cents for a wallet that counts in billionths. The consent page then shows nothing wrong.

## Decision

A grant request is refused with a 400 when an item that creates outgoing payments has a debit limit in another asset than the item's wallet address. The answer names the asset the limit must be in. The check is made where the node resolves the item's wallet address, before the owner is asked.

## Accepted

- A request the specs allow is refused. A client that sent it would get a grant no payment could use anyway.
- A debit limit on an item that only reads or lists is not checked. It caps no payment there.

## Rejected

- *Check a limit's asset only when a payment is made:* the owner approves a grant that can never pay, the app learns why only when it pays, and the owner never learns.
- *Refuse a receive limit in another asset than the receiver the grant names:* checking it means reading the receiver from its node while answering the request, which can be slow or fail. That read also belongs to the consent page, where the owner sees it.
