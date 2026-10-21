# Names resolve one address family at a time, through the system's resolver

**Areas:** [Peer protocol](../areas/peer-protocol.md#outbound-requests), [Node](../areas/node.md#a-chosen-relay).

## Facts

- Every outbound connection resolves its name in one place, the peer client's resolver: peer requests over QUIC and HTTPS, the chosen relay, Open Payments to other wallets, exchange-rate sources and the settlement schemes' WebSockets ([Peer protocol](../areas/peer-protocol.md#outbound-requests)).
- Release binaries are static musl builds ([Release](../areas/release.md)). Asked for both families in one call, musl's `getaddrinfo` sends the A and AAAA queries together and fails the whole name when either answer is an error. glibc keeps the answer that succeeded.
- Some home routers' resolvers answer one family with NXDOMAIN while the other resolves: a dnsmasq on a consumer gateway answered AAAA queries for the DNSSEC-signed `dassiepay.net` so, where the authoritative servers and public resolvers answer that no such record exists. Behind such a router a static node could not resolve its relay at all, and so was never reachable, while the same binary reached it at once with a public resolver.
- Periphery nodes run on exactly these networks ([Network](../areas/network.md#periphery-nodes)), so the node has to work despite them, though the router is at fault.
- The dev environment and the tests rely on the system's name resolution: `/etc/hosts`, nsswitch, and names the machine itself serves.
- The workspace forbids `unsafe` code ([Engineering](../engineering.md#pragmatic-tigerstyle)). A family-specific lookup is not in std or tokio, whose `lookup_host` asks for both families in one call.

## Decision

The resolver looks each name up twice at once, IPv4 only and IPv6 only, each through the system's `getaddrinfo` on a blocking thread, by way of the `dns-lookup` crate (`crates/dassie-node/src/peer_client.rs`). It merges what the two found:

- **It fails only if both lookups failed,** with the IPv4 lookup's error, and finds nothing only if neither found an address the node may use. The private-address guard applies to each family's addresses as before.
- **Each family keeps a bounded number of addresses,** and the merged list alternates the two, IPv4 first.
- **The IPv4 lookup is always waited for in full,** as a lookup of both families in one call was. IPv4 is what a home network behind a NAT reaches surely, so cutting it short could leave only IPv6 addresses the node cannot reach: the two records have their own TTLs, and an A record no longer cached can take a full recursion while the AAAA record answers at once.
- **The IPv6 lookup is waited for only briefly once IPv4 has given addresses the node may use,** and counts as failed if it is late. A resolver that drops AAAA queries would otherwise hold the lookup until its own timeout, which can exceed the time a dial has. IPv4 addresses the guard refuses do not count, so a name with only private A records still waits for its AAAA.
- **IPv6 lookups running at once are bounded.** One no longer waited for runs on until the system's resolver gives up; past the bound, a name is looked up for IPv4 only.

A dial that fails before sending anything, for want of an address, logs the host and why at debug, once per dial.

## Accepted

- **Two lookups instead of one,** each on a blocking thread of tokio's pool, which the node's file IO and certificate reloads share. An IPv6 lookup no longer waited for keeps its thread until the system's resolver gives up, so a resolver that never answers AAAA holds threads for longer than the requests that started them. The bound on IPv6 lookups running at once caps those threads well below the pool's size; while it is reached, names resolve to IPv4 only.
- **The order between the families is ours, not the system's.** `getaddrinfo` sorts both families together by the machine's routes (RFC 6724) only when it is asked for both. IPv4 first is what a home network behind a NAT reaches surely; the HTTPS client still falls back to the other family quickly, and a QUIC dial tries the addresses in turn.
- **An IPv6 answer slower than the grace is dropped** while IPv4 has addresses the node may use, so such a node is dialled over IPv4 only. A late IPv4 answer is never dropped, so the node waits as long as before for a slow A record.
- **One more dependency,** a small crate that holds the FFI, beside `listenfd`, which is there for the same reason.

## Rejected

- *`libc::getaddrinfo` directly:* the same calls, but `unsafe` in a crate that forbids it, for an FFI that `dns-lookup` already wraps, frees and maps to errors.
- *A DNS client of our own, such as hickory-resolver configured from `/etc/resolv.conf`:* it reads neither `/etc/hosts` nor nsswitch, so names the machine resolves another way (container and VPN setups, local overrides) stop resolving, and it brings a far larger dependency.
- *A grace for whichever family answers second:* when the AAAA record is cached and the A record is not, the A lookup is cut short, and an IPv4-only network is left with addresses it cannot reach, where one lookup of both families waited for A.
- *Retrying with IPv4 only when the combined lookup fails:* it fixes the router seen, but not one that fails the A query, and it costs a second round trip, after a resolver timeout in the worst case, every time.
- *Building releases against glibc:* the static binary is how a release runs on any Linux without shared libraries ([Release](../areas/release.md)), and glibc's resolver still waits for both answers, so a family that is never answered still holds the lookup.
