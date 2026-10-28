/**
 * What the end-to-end tests of swaps and the market maker share
 * (spec/areas/rate-feed.md#market-maker, spec/decisions/peer-swaps.md):
 * the scenario's nodes, reading what they hold across their hubs and
 * owner books, the peers' snapshots a node holds and what they promise,
 * the fake ledger's pool, and writing the market maker's config. A
 * scenario founds an XRP hub and a DSUSD hub; its core nodes, the
 * targets, join both, with XRP and DSUSD, and so does the market maker's
 * one periphery node, pinned to every core node.
 *
 * @module
 */

import { assert } from "@std/assert";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { FundsKept, type PeerSnapshot } from "@dassie/rpc/proto";
import {
  hubLedger,
  hubOwnerLedger,
  hubsOf,
  joins,
  type Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

/** The ledgers of the XRP hub and of the DSUSD hub, which the market
 * maker swaps between and the core nodes convert between. */
export const XRP_HUB = "hub-xrpl-testnet-xrp+xrp";
export const DSUSD_HUB = "hub-xrpl-testnet-dsusd+usd";
/** Their settlement methods, by the schemes over the hubs: what a
 * snapshot quotes and a swap names. */
export const XRP_METHOD = "hub-xrpl-testnet-xrp";
export const DSUSD_METHOD = "hub-xrpl-testnet-dsusd";
/** The owner books that fund them, on each node's XRP Ledger account. */
export const XRP_BOOK = "xrpl-testnet-xrp+xrp";
export const DSUSD_BOOK = "xrpl-testnet-dsusd+usd";
/** An asset, as a node holds it: its hub's ledger and its owner book. */
export type Asset = readonly [hub: string, book: string];
export const XRP: Asset = [XRP_HUB, XRP_BOOK];
export const DSUSD: Asset = [DSUSD_HUB, DSUSD_BOOK];
/** DSUSD's issuer on the testnet, which the fake ledger issues it at. */
export const ISSUER = "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M";
/** DSUSD's currency code as the ledger holds it: its letters, padded. */
const DSUSD_CURRENCY = "4453555344000000000000000000000000000000";
/** A whole XRP or dollar in the books' units, which count at scale 9. */
export const UNIT = 1_000_000_000n;
/** The largest packet, in the books' units: a tenth of a unit. */
export const LARGEST_PACKET = 100_000_000n;
/** What the market maker pays through a core node in one trade: 2 dollars. */
export const TRADE = 2n * UNIT;
/**
 * How long a swap the owner asks for may take: the node sends for a minute
 * at most, then waits for what it has in flight (`SwapWithPeer`).
 */
export const SWAP_TIMEOUT_MS = 180_000;
/**
 * The least gain a trade must make: 1%. A core node holds about a
 * twentieth of its value in DSUSD, so its curve takes DSUSD in at up to its
 * cap, 5% above spot, less each packet's own impact: a trade clears the
 * margin after the pool's fee by a few percent.
 */
const MARGIN_PPM = 10_000;
/** How far the pool's price may move against a swap: 1%. */
const SLIPPAGE_PPM = 10_000;
/**
 * The band of the market maker's XRP around what it holds at the start,
 * unless a test names another: above it, after two trades, the market
 * maker swaps its XRP back into DSUSD through the pool.
 */
const XRP_BAND = 6n * UNIT;
/** The band of its DSUSD, in dollars: room for every trade a run makes. */
const DSUSD_LOW = 5n * UNIT;
const DSUSD_HIGH = 40n * UNIT;
/** How long the fake ledger may take to answer `amm_info`. */
const AMM_INFO_TIMEOUT_MS = 10_000;

/** The nodes of a market maker's scenario. */
export interface MarketMakerNodes {
  /** The core nodes, its targets, members of both hubs. */
  readonly cores: ScenarioNode[];
  /** The market maker's node, a periphery node in both hubs. */
  readonly makerNode: ScenarioNode;
}

/** The scenario's core nodes and the market maker's node: the members of
 * both hubs, the periphery node among them the market maker's. */
export function marketMakerNodes(scenario: Scenario): MarketMakerNodes {
  const [xrpHub, dsusdHub] = hubsOf(scenario.def);
  assert(xrpHub && dsusdHub, "the scenario has no XRP and DSUSD hubs");
  const inBoth = scenario.nodes.filter((node) =>
    joins(scenario.def, xrpHub, node.index) &&
    joins(scenario.def, dsusdHub, node.index)
  );
  const cores = inBoth.filter((node) => !node.periphery);
  const makers = inBoth.filter((node) => node.periphery);
  assert(cores.length > 0 && makers.length === 1, "the market maker's nodes");
  assert(
    hubLedger(xrpHub) === XRP_HUB && hubLedger(dsusdHub) === DSUSD_HUB &&
      hubOwnerLedger(xrpHub) === XRP_BOOK &&
      hubOwnerLedger(dsusdHub) === DSUSD_BOOK,
    "the hubs' ledgers",
  );
  return { cores, makerNode: makers[0]! };
}

/** A node's ledger, as its owner sees it. */
export async function ledger(node: ScenarioNode, id: string) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  return ledgers.find((entry) => entry.id === id);
}

/** What the owner holds on a node's ledger, by its books. */
export async function owner(node: ScenarioNode, id: string): Promise<bigint> {
  const found = await ledger(node, id);
  assert(found, `${node.name} has no ${id}`);
  return found.ownerBalance;
}

/**
 * What the owner holds of `asset` on a node: in its hub and in the owner
 * book that funds it, between which the node moves funds by itself
 * (spec/areas/hubs.md#in-the-node). Read once the moves are done, as
 * settling over the hubs waits for.
 */
export async function held(
  node: ScenarioNode,
  [hub, book]: Asset,
): Promise<bigint> {
  return (await owner(node, hub)) + (await owner(node, book));
}

/** Whether `node` has fresh rates for XRP and dollars, which its snapshot
 * prices its hubs' ledgers at. */
export async function freshRates(node: ScenarioNode): Promise<boolean> {
  const { rates } = await node.rpc.call("listLedgers", {});
  return ["XRP", "USD"].every((code) =>
    rates.some((rate) => rate.assetCode === code && !rate.stale)
  );
}

/**
 * The snapshot of `of` that `node` holds, if any, with both hubs' methods
 * named by the schemes `node` runs (`ListPeerSnapshots`).
 */
export async function peerSnapshot(
  node: ScenarioNode,
  of: ScenarioNode,
): Promise<PeerSnapshot | undefined> {
  const { snapshots } = await node.rpc.call("listPeerSnapshots", {});
  const snapshot = snapshots.find((listed) => listed.node === of.id);
  const schemes = snapshot?.methods.map((method) => method.scheme) ?? [];
  const both = [XRP_METHOD, DSUSD_METHOD].every((scheme) =>
    schemes.includes(scheme)
  );
  return both ? snapshot : undefined;
}

/** A quote's word: a mantissa in its low 24 bits and an exponent in its
 * top 8, worth `mantissa · 2^(exponent − 150)`. */
function quote(word: number): { mantissa: bigint; exponent: number } {
  return { mantissa: BigInt(word & 0xff_ffff), exponent: (word >>> 24) - 150 };
}

/** `numerator · 2^shift / denominator`, rounded down. */
function shifted(numerator: bigint, denominator: bigint, shift: number) {
  return shift >= 0
    ? (numerator << BigInt(shift)) / denominator
    : numerator / (denominator << BigInt(-shift));
}

/**
 * The least a swap of `amount`, sent as one packet from the snapshot's
 * state, gets on the method `to` for `amount` on the method `from`, as the
 * snapshot promises it (spec/areas/rate-feed.md#reading-a-snapshot): the
 * rate of the step the packet's last unit falls in, `from`'s bid less that
 * level's drop, times one less the drop of `to`'s ask level, over `to`'s
 * ask, rounded down. `undefined` when the snapshot does not price the pair
 * or the packet is beyond what its ladders cover. Written apart from the
 * node's reader, so that a test checks what came back against the
 * snapshot itself.
 */
export function promised(
  snapshot: PeerSnapshot,
  from: string,
  to: string,
  amount: bigint,
): bigint | undefined {
  const of = (scheme: string) =>
    snapshot.methods.find((method) => method.scheme === scheme);
  const [paid, got] = [of(from), of(to)];
  if (!paid || !got || paid.bid === 0 || got.ask === 0) return undefined;
  const [bid, ask] = [quote(paid.bid), quote(got.ask)];
  // The ask ladder counts what goes out; what comes in reaches each of its
  // levels at the top-of-book price.
  const askTops = got.askLadder.map((level) =>
    shifted(level * ask.mantissa, bid.mantissa, ask.exponent - bid.exponent)
  );
  const levelIn = paid.bidLadder.findIndex((top) => amount <= top);
  const levelOut = askTops.findIndex((top) => amount <= top);
  if (levelIn < 0 || levelOut < 0) return undefined;
  const keep = (level: number) => 10_000n - BigInt(snapshot.gridBp[level]!);
  return shifted(
    amount * bid.mantissa * keep(levelIn) * keep(levelOut),
    ask.mantissa * 100_000_000n,
    bid.exponent - ask.exponent,
  );
}

/**
 * What node A gains, in parts per million, by a vanishingly small swap with
 * node B that pays on `pay` and is paid back on `back`: B's bid on `pay`
 * over its ask on `back` (B's snapshot `theirs`), valued at A's own quotes,
 * its bid on `back` over its bid on `pay` (A's snapshot `ours`), less one.
 * A swaps while it clears A's costs (spec/decisions/peer-swaps.md): at the
 * top of the book, with no fee or spread, this is what A weighs them
 * against. `undefined` when a snapshot does not price the pair.
 */
export function gainPpm(
  ours: PeerSnapshot,
  theirs: PeerSnapshot,
  pay: string,
  back: string,
): bigint | undefined {
  const of = (snapshot: PeerSnapshot, scheme: string) =>
    snapshot.methods.find((method) => method.scheme === scheme);
  const words = [
    of(theirs, pay)?.bid,
    of(ours, back)?.bid,
    of(theirs, back)?.ask,
    of(ours, pay)?.bid,
  ];
  if (words.some((word) => !word)) return undefined;
  const [up1, up2, down1, down2] = words.map((word) => quote(word!));
  return shifted(
    up1!.mantissa * up2!.mantissa * 1_000_000n,
    down1!.mantissa * down2!.mantissa,
    up1!.exponent + up2!.exponent - down1!.exponent - down2!.exponent,
  ) - 1_000_000n;
}

/** A decimal, such as `"49995.5"` or `"1e-3"`, at `scale` decimals,
 * rounded down. */
function decimal(text: string, scale: number): bigint {
  const match = /^(\d+)(?:\.(\d+))?(?:e(-?\d+))?$/.exec(text);
  assert(match, `not a decimal: ${text}`);
  const digits = BigInt(`${match[1]}${match[2] ?? ""}`);
  const shift = scale - (match[2]?.length ?? 0) + Number(match[3] ?? 0);
  return shift >= 0
    ? digits * 10n ** BigInt(shift)
    : digits / 10n ** BigInt(-shift);
}

/** The pool of XRP and DSUSD, as a validated ledger has it, in the books'
 * units. */
export interface Pool {
  readonly xrp: bigint;
  readonly dsusd: bigint;
}

/** Reads the pool with `amm_info` from the fake ledger. */
export async function pool(scenario: Scenario): Promise<Pool> {
  assert(scenario.ledger, "the scenario runs no fake ledger");
  // Node's own WebSocket: the fake ledger is on `ws://`, on loopback.
  const socket = new WebSocket(scenario.ledger.url);
  try {
    const answer = await new Promise<string>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("amm_info: no answer")),
        AMM_INFO_TIMEOUT_MS,
      );
      socket.addEventListener("error", () => {
        clearTimeout(timer);
        reject(new Error("amm_info: the fake ledger is not there"));
      });
      socket.addEventListener("open", () =>
        socket.send(JSON.stringify({
          id: 1,
          command: "amm_info",
          asset: { currency: "XRP" },
          asset2: { currency: DSUSD_CURRENCY, issuer: ISSUER },
          ledger_index: "validated",
        })));
      socket.addEventListener("message", (event) => {
        const text = String(event.data);
        if (JSON.parse(text).id !== 1) return;
        clearTimeout(timer);
        resolve(text);
      });
    });
    const amm = JSON.parse(answer).result?.amm;
    assert(amm, `amm_info: ${answer}`);
    return {
      xrp: BigInt(amm.amount) * 1_000n,
      dsusd: decimal(amm.amount2.value, 9),
    };
  } finally {
    socket.close();
  }
}

/** What a node holds, in the books' units: XRP and DSUSD, each in its hub
 * and its owner book. */
export interface Holdings {
  readonly xrp: bigint;
  readonly dsusd: bigint;
}

export async function holdings(node: ScenarioNode): Promise<Holdings> {
  return { xrp: await held(node, XRP), dsusd: await held(node, DSUSD) };
}

/** `held`'s value in dollars at `pool`'s price, times what the pool holds
 * of XRP: exact, and as good to compare at one price. */
export function value(held: Holdings, at: Pool): bigint {
  return held.dsusd * at.xrp + held.xrp * at.dsusd;
}

/** A value from {@linkcode value}, in dollars, for messages. */
export function dollars(scaled: bigint, at: Pool): string {
  const micros = scaled * 1_000_000n / at.xrp / UNIT;
  return `${micros / 1_000_000n}.${`${micros % 1_000_000n}`.padStart(6, "0")}`;
}

/**
 * Writes the market maker's config, and its node's passphrase beside it,
 * into `dir`: its node, with each asset's hub ledger and the owner book
 * that funds it, and the pool on the fake ledger. Its targets are its
 * node's peers. The band of its XRP is `xrpBand` around what the node
 * holds now, in its hub and its book. Returns the config's path.
 */
export async function writeConfig(
  scenario: Scenario,
  dir: string,
  node: ScenarioNode,
  xrpBand = XRP_BAND,
): Promise<string> {
  const phrase = join(dir, `${node.name}.phrase`);
  await writeFile(phrase, `${node.identity.phrase}\n`, { mode: 0o600 });
  // Its rebalances spend from the owner books, so what the node is paid
  // must reach them: its owner keeps it on the own account, which each
  // hub's funding sweeps beyond the ceiling, as the VM's setup does with
  // `--keep-funds account` (spec/areas/rate-feed.md#bands).
  await node.rpc.call("setFundsKept", {
    name: "",
    fundsKept: FundsKept.OWN_ACCOUNT,
  });
  const xrpHeld = await held(node, XRP);
  const config = {
    socket: node.socket,
    passphraseFile: phrase,
    xrpl: {
      server: scenario.ledger!.url,
      token: { currency: "DSUSD", issuer: ISSUER },
    },
    xrp: {
      ledger: XRP_HUB,
      ownerBook: XRP_BOOK,
      tradeSize: Number(TRADE),
      band: {
        low: Number(xrpHeld - xrpBand),
        high: Number(xrpHeld + xrpBand),
      },
    },
    token: {
      ledger: DSUSD_HUB,
      ownerBook: DSUSD_BOOK,
      tradeSize: Number(TRADE),
      band: { low: Number(DSUSD_LOW), high: Number(DSUSD_HIGH) },
    },
    marginPpm: MARGIN_PPM,
    slippagePpm: SLIPPAGE_PPM,
    // The dev CA, as the scenario's nodes trust it
    // (`DASSIE_EXTRA_CA_FILE`): a host such as CI's build sandbox has no
    // roots of its own, though the fake ledger is on `ws://`.
    caFile: scenario.caFile,
    allowPrivate: true,
  };
  const file = join(dir, "config.json");
  await writeFile(file, JSON.stringify(config));
  return file;
}
