# A rate feed names the settlement methods a node's peers pay it on

**Areas:** [Rate feed](../areas/rate-feed.md#what-a-feed-says), [Connector](../areas/connector.md#inventory-pricing). Built, as feed version 2.

## Facts

- Peers settle only over settlement hubs ([hubs-only](hubs-only.md)). The schemes on the underlying ledgers are the owner's books: they hold the owner's funds and fund the hubs, and peer with nobody. The schemes a node peers on are the ones its link state offers, each a settlement method ([settlement-methods](settlement-methods.md)).
- The price table prices every ledger the node runs, owner books included, since the curve counts each asset whole, a hub's ledger with the book that funds it ([Connector](../areas/connector.md#inventory-pricing)).
- A market maker trades through a node by paying it on one method and being paid on another ([Rate feed](../areas/rate-feed.md#market-maker)). A rate into or out of an owner book is one no packet from a peer can get.
- A ledger's ID is its scheme's and its asset's, `<scheme>+<asset>`, and each scheme serves one ledger ([hub-scheme-names](hub-scheme-names.md)). A peer and a market maker know a node's methods by their scheme IDs.
- The feed's wire format is frozen within its version ([Rate feed](../areas/rate-feed.md#wire-format)). Version 1 named ledgers, and released nodes speak it.
- A feed is one datagram of at most `MAX_DATAGRAM_BYTES`, with a rate for each ordered pair it names: the rates grow with the square of what it names. Eight entries of the longest IDs fit; sixteen would take 240 rates, twice the datagram for the rates alone.

## Decision

- **A feed names settlement methods,** by their schemes' IDs: each method the node offers its peers whose ledger the table prices, with its largest packet, and a rate for each ordered pair of them it converts between. An owner book, and any ledger no peer can pay the node on, is left out, though the table prices it.
- **A new feed version, 2,** signed under a new OID, `…1399.3.2`, since version 1, which named ledgers, is spoken by released nodes. A market maker of an earlier release refuses a version 2 feed and holds until it is updated.
- **At most `MAX_METHODS` methods, with IDs of at most `MAX_METHOD_ID_BYTES`,** the most a datagram holds with every pair priced, a constant assertion checks, and one more would not fit. That is more than all the schemes of either realm, owner books included, should direct settlement ever come back. A node offers only schemes of its realm, so a constant assertion over the schemes it can run says that every method it offers fits: the node never cuts a feed short, and a realm with more needs a new version.
- **The market maker reads the rate between its two hubs' methods,** the schemes of the hub ledgers its config names, so its config is unchanged.

## Accepted

- **A rollout in which subscribers and nodes disagree:** a market maker that has not updated holds until it has, and one that has finds no feed on a node that has not.
- **Fewer than sixteen methods per feed.** A node in more hubs than that needs another layout, and so a new version.
- **A method on a ledger the table does not price is left out,** as an unpriced ledger was, so a feed does not say which methods a node offers but cannot convert on now.

## Rejected

- *Ledgers by ID, as version 1:* owner books are listed, and rates into and out of them, which no peer can use, and a subscriber has to know that a hub's ledger is its method's.
- *Methods under version 1:* subscribers of released versions would read a feed whose entries mean something else, and its signature would pass for one of the old layout's.
- *Cutting the list at the bound, as version 1 did:* a feed would leave out what a node converts at without saying so.
- *Sixteen methods, or more,* with rates per method rather than per pair: rates per pair are what the table prices, exactly, and a feed promises no more than a packet gets.
