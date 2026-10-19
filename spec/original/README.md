# The original Dassie

These chapters describe the original Dassie, the TypeScript proof of concept that the current implementation replaced. They are a quick reference for what the original did, bugs included, and they are not normative: everything in them that still holds for the current implementation is also stated in the [area docs](../README.md#areas).

| # | Chapter | What it covers |
|---|---|---|
| 1 | [Overview](01-overview.md) | What the original is, concepts, system context, the life of a node, feature inventory |
| 2 | [Node runtime](02-node-runtime.md) | Process model and startup, configuration, filesystem and env vars, database schema, HTTP/HTTPS/IPC, TLS and ACME, authentication, RPC procedures, CLI, logging, security model, the reactive actor model and supporting libraries |
| 3 | [Peer network](03-peer-network.md) | Node identity and keys, the peer protocol v0 (wire format, transport, authentication, handlers), node table, link state and flooding, membership, peering, uplink addresses, routing, BTP server |
| 4 | [Interledger](04-interledger.md) | ILPv4 packets, the OER library, node addressing, the connector pipeline, exchange rates and conversion, ILDCP, local endpoints, ILP-over-HTTP, Open Payments and payment pointers, BTP schemas, STREAM |
| 5 | [Accounting and settlement](05-accounting-and-settlement.md) | The double-entry internal ledger, how packets and settlements become transfers, the settlement-scheme interface, the settlement trigger, stub and XRPL modules |
| 6 | [User interfaces and tooling](06-user-interfaces-and-tooling.md) | The web wallet page by page, the dev dashboard, the website and STREAM toy, release build, installer, updater, the multi-node dev environment |

## Sources

The original implementation is the `main` branch of this repository at commit `e962ab78` ("chore(x509): allow importing node built-in modules in tests", 2025-11-08). The `rewrite` branch has no shared history with it. Use `git show e962ab78:<path>` to read a cited file.

Source citations use paths relative to that tree. Each chapter's preamble says what its short paths are relative to (`APP/` and `AD/` both mean `packages/app-dassie/src/`). The original's own design notes live in `docs/implementation-notes/` there. Their content is folded into the chapters, including where the code disagrees with them.

Test vectors come from the original's fixtures unless marked otherwise. [3 §5.5](03-peer-network.md) contains vectors first computed with an independent encoder written from this spec; they have since been confirmed against the original.

## Conventions

- **Behaviour vs. intent.** The chapters describe what the original code *does*, including its bugs. Where that differs from its design notes, from a standard, or from obvious intent, the text says so: **Deviation** (differs from an RFC or reference implementation), **Open question / known gap**, or "Observed behaviour".
- **Gap IDs.** Each chapter numbers its gaps with its own prefix: `RT-n` (2), `NET-n` and `OER-q` (3), `ILP-n` (4), `ACC-n` (5), `UI-n` (6). Tests of the current implementation that fix one of these gaps are named after it.
- **Normative language.** **MUST** marks behaviour that appears on the wire or changes balances, so an implementation that wants compatibility has to match it. Which surfaces the current implementation keeps compatible is stated in the [overview](../overview.md).
- **Section references** have the form `[3 §10.2](03-peer-network.md)`: chapter 3, section 10.2.
- **Byte notation.** `||` or `‖` is concatenation, hex is lowercase unless quoted from a protocol that uses uppercase, and `LEN(x)` is an OER length determinant.
