# The original Dassie

These chapters describe the original Dassie, the TypeScript proof of concept that the current implementation replaced. They were written so that it could be rebuilt without reading its code. They are a quick reference for what the original did, bugs included, and they are not normative: everything in them that still holds for the current implementation is also stated in the [area docs](../README.md#areas).

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
- **Gap IDs.** Each chapter numbers its gaps with its own prefix: `RT-n` (2), `NET-n` and `OER-q` (3), `ILP-n` (4), `ACC-n` (5), `UI-n` (6). Code, area docs and decisions cite them by these IDs.
- **Normative language.** **MUST** marks behaviour that appears on the wire or changes balances, so an implementation that wants compatibility has to match it. Which surfaces the current implementation keeps compatible is stated in the [overview](../overview.md).
- **Section references** have the form `[3 §10.2](03-peer-network.md)`: chapter 3, section 10.2.
- **Byte notation.** `||` or `‖` is concatenation, hex is lowercase unless quoted from a protocol that uses uppercase, and `LEN(x)` is an OER length determinant.

## How the rewrite treated the gaps

The chapters record 124 numbered gaps:

| Prefix | Chapter | Count |
|---|---|---|
| RT | [2 §20](02-node-runtime.md) | 28 |
| NET | [3 §19](03-peer-network.md) | 36 |
| OER-q | [3 §19](03-peer-network.md) | 3 |
| ILP | [4 §14](04-interledger.md) | 14 |
| ACC | [5 §10](05-accounting-and-settlement.md) | 26 |
| UI | [6 §9](06-user-interfaces-and-tooling.md) | 17 |

Many more are marked inline as "Deviation" or "Open question / known gap".

**Every gap is fixed,** unless the original's behaviour is kept on purpose (the [overview](../overview.md) lists those surfaces) or a decision says otherwise. "Fixed" means:

- implement what the original's design notes or code comments say was intended; or, where there is no stated intent,
- implement the governing standard (X.696, ILPv4, RFC 29 STREAM, RFC 31 ILDCP, RFC 23 BTP, RFC 35 ILP-over-HTTP, Open Payments) or the obvious safe behaviour.

Every fix has a test that would have failed against the original's behaviour.

The gaps that a decision covers:

| Decision | Gaps |
|---|---|
| [D2](../decisions/D02-peer-protocol-v0-or-v1.md) | NET-3, NET-4, NET-8, NET-9, NET-10, NET-11, NET-26, NET-27, NET-28 |
| [D3](../decisions/D03-database-continuity.md) | RT-25 (storage format), ACC-5 and ACC-6 (no carried-over totals) |
| [D4](../decisions/D04-network-membership-economics.md) | NET-15, NET-16 (doc fix), NET-17, NET-18, NET-19, NET-29 |
| [D5](../decisions/D05-ui-and-cli-rpc-contract.md) | RT-5, RT-6, RT-26, RT-27, UI-7 |
| [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md) | ACC-1, ACC-9, ACC-24, ILP-7, ILP-10 (asset), UI-2, UI-3 |
| [D7](../decisions/D07-exchange-rates-fees-and-spread.md) | ACC-3, ILP-6, ILP-14 |
| [D8](../decisions/D08-real-money.md) | ACC-13, ACC-19, ACC-20, ACC-21, ACC-22 |
| [D9](../decisions/D09-open-payments-fully-and-no-spsp.md) | ILP-9, ILP-10 |
| [D10](../decisions/D10-secrets-at-rest.md) | RT-10, RT-11, RT-25 |
| [D11](../decisions/D11-release-integrity.md) | RT-18, RT-19, UI-10, UI-11, UI-12, UI-13 |
| [D12](../decisions/D12-peering-lifecycle-and-liveness.md) | NET-12, NET-13, NET-21, NET-22, NET-23, NET-35 |
| [D13](../decisions/D13-connector-timing-and-packet-limits.md) | ILP-1, ILP-2, ILP-3, ILP-4, ILP-5, ILP-8, NET-33, NET-34 |
| [D15](../decisions/D15-bootstrap-network-for-v1.md) | NET-19 |
