# Implementation Notes: Internal Ledger

Dassie uses an internal ledger to track funds. It implements double-entry accounting using a data model inspired by [Tigerbeetle](https://tigerbeetle.com/).

## Data Model

Dassie uses transfers to record changes to the internal ledger. Each transfer has an amount and debits one account and credits another account. Transfers can either be posted immediately or first spend time in a "pending" state and later be posted or cancelled.

The accounts that transfers are being applied to are identified by a string path, for example `xrp:assets/settlement`. The first element is the `LedgerId`, followed by a colon (`:`). The rest of the string is a hierarchical identifier using slash (`/`) as the separator.

## Chart of Accounts

There are several types of account in Dassie:

- Asset account: Balance sheet. Debits increase it, credits decrease it.
- Liability account: Balance sheet. Credits increase it, debits decrease it.
- Equity account: Balance sheet. Credits increase it, debits decrease it.
- Revenue account: Profit/loss. Credits increase it, debits decrease it.
- Expense account: Profit/loss. Debits increase it, credits decrease it.
- Contra account: Used to adjust another account.

Here is a list of Dassie's accounts

```
assets/…
  settlement          Assets held on the underlying ledger
  interledger/[peer]  Amount owed to us by the peer

liabilities/…

contra/…
  trust/[peer]        Represents trust extended to a peer and reduces the
                      interledger asset account (possibly turning it negative
                      and into a liability)

equity/…
  owner               Owner's equity account
  suspense            Suspense account (used to keep the assets/settlement
                      account matching the underlying ledger)

revenue/…
  fees                Fees earned by the node in the course of its operation
  fx                  Foreign exchange account

expenses/…
  fees                Fees paid by the node (such as node registration)
  fx                  Foreign exchange account
```

## Transaction Types

#### Interledger payment from peer A to peer B (same currency)

```
Transfer 1:
 Dr. xrp:assets/interledger/peerA
 Cr. xrp:assets/interledger/peerB

Transfer 2:
 Dr. xrp:liabilities/peerA/interledger
 Cr. xrp:revenue/fees
```

#### Interledger payment from peer A to peer B (cross-currency)

```
Transfer 1:
 Dr. xrp:assets/interledger/peerA
 Cr. xrp:revenue/fx

Transfer 2:
 Dr. xrp:assets/interledger/peerA
 Cr. xrp:revenue/fees

Transfer 3:
 Dr. btc:expenses/fx
 Cr. btc:assets/interledger/peerB
```

#### Settlement from peer A

```
Transfer 1:
 Dr. xrp:assets/settlement
 Cr. xrp:assets/interledger/peerA
```
