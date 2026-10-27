/**
 * What the market maker's end-to-end tests share
 * (spec/areas/rate-feed.md#market-maker): the scenario's nodes, reading
 * what they hold across their hubs and owner books, the fake ledger's
 * pool, and writing the market maker's config. A scenario founds an XRP
 * hub and a DSUSD hub; its core nodes, the targets, join both, with XRP
 * and DSUSD, and the market maker's two periphery nodes, pinned to every
 * core node, join one each: one with XRP, one with DSUSD.
 *
 * @module
 */

import { assert } from "@std/assert";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Agent, fetch } from "undici";
import {
  hubLedger,
  hubOwnerLedger,
  hubsOf,
  joins,
  type Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

/** The ledgers of the XRP hub and of the DSUSD hub, which the market
 * maker trades on and the core nodes convert between. */
export const XRP_HUB = "hub-xrpl-testnet-xrp+xrp";
export const DSUSD_HUB = "hub-xrpl-testnet-dsusd+usd";
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
/** What the market maker sends through a core node in one trade: 2 dollars. */
export const TRADE = 2n * UNIT;
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
 * The XRP node's band around what it holds at the start, unless a test
 * names another: above it, after two trades, the market maker swaps its XRP
 * back into DSUSD through the pool.
 */
const XRP_BAND = 6n * UNIT;
/** The DSUSD node's band, in dollars: room for every trade a run makes. */
const DSUSD_LOW = 5n * UNIT;
const DSUSD_HIGH = 40n * UNIT;
/** How long the fake ledger may take to answer `amm_info`. */
const AMM_INFO_TIMEOUT_MS = 10_000;

/**
 * Why the market maker's tests do not check that it moves its targets
 * toward balance, while they do not: a core node prices the ledger of each
 * of its hubs apart from the owner book that funds it
 * (spec/areas/connector.md#inventory-pricing), so its curve follows what
 * its two hub accounts hold, rather than what it holds of each asset. Its
 * feed then offers more than spot for XRP, the asset it holds most of, and
 * the market maker trades it further from balance
 * (spec/areas/rate-feed.md#market-maker). Meanwhile the test with one
 * target checks only that a trade crosses the core node over both hubs,
 * whichever way, and the test with two is skipped. Both run whole once
 * inventory pricing counts each asset whole (spec/roadmap.md#hubs-only).
 */
export const PRICED_APART: string | undefined =
  "a core node prices its hubs' ledgers apart from their owner books (spec/roadmap.md#hubs-only)";

/** The nodes of a market maker's scenario. */
export interface MarketMakerNodes {
  /** The core nodes, its targets, members of both hubs. */
  readonly cores: ScenarioNode[];
  /** The market maker's node in the XRP hub. */
  readonly xrpNode: ScenarioNode;
  /** The market maker's node in the DSUSD hub. */
  readonly dsusdNode: ScenarioNode;
}

/** The scenario's core nodes and the market maker's two, by the hubs each
 * joins: the cores both, the market maker's one each. */
export function marketMakerNodes(scenario: Scenario): MarketMakerNodes {
  const [xrpHub, dsusdHub] = hubsOf(scenario.def);
  assert(xrpHub && dsusdHub, "the scenario has no XRP and DSUSD hubs");
  const inXrp = (node: ScenarioNode) => joins(scenario.def, xrpHub, node.index);
  const inDsusd = (node: ScenarioNode) =>
    joins(scenario.def, dsusdHub, node.index);
  const cores = scenario.nodes.filter((node) => inXrp(node) && inDsusd(node));
  const xrpNode = scenario.nodes.find((node) => inXrp(node) && !inDsusd(node));
  const dsusdNode = scenario.nodes.find((node) =>
    inDsusd(node) && !inXrp(node)
  );
  assert(cores.length > 0 && xrpNode && dsusdNode, "the market maker's nodes");
  assert(
    hubLedger(xrpHub) === XRP_HUB && hubLedger(dsusdHub) === DSUSD_HUB &&
      hubOwnerLedger(xrpHub) === XRP_BOOK &&
      hubOwnerLedger(dsusdHub) === DSUSD_BOOK,
    "the hubs' ledgers",
  );
  return { cores, xrpNode, dsusdNode };
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

/**
 * The port `node`'s discovery document names for its feed, if it names one.
 */
export async function feedPort(
  scenario: Scenario,
  node: ScenarioNode,
): Promise<number | undefined> {
  const agent = new Agent({
    connect: { ca: await readFile(scenario.caFile, "utf8") },
  });
  try {
    const response = await fetch(`${node.localUrl}/.well-known/dassie/node`, {
      dispatcher: agent,
    });
    const document = await response.json() as { feedPort?: number };
    return document.feedPort;
  } finally {
    await agent.close();
  }
}

/** Whether `node` has fresh rates for XRP and dollars, which its feed
 * prices its hubs' ledgers at. */
export async function freshRates(node: ScenarioNode): Promise<boolean> {
  const { rates } = await node.rpc.call("listLedgers", {});
  return ["XRP", "USD"].every((code) =>
    rates.some((rate) => rate.assetCode === code && !rate.stale)
  );
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

/** What the market maker holds, in the books' units: XRP in its XRP
 * node's hub and owner book and in its DSUSD node's owner book of XRP,
 * which pays that node's gas, and DSUSD in its DSUSD node's hub and owner
 * book. */
export interface Holdings {
  readonly xrp: bigint;
  readonly dsusd: bigint;
}

export async function holdings(
  xrpNode: ScenarioNode,
  dsusdNode: ScenarioNode,
): Promise<Holdings> {
  return {
    xrp: (await held(xrpNode, XRP)) + (await owner(dsusdNode, XRP_BOOK)),
    dsusd: await held(dsusdNode, DSUSD),
  };
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
 * Writes the market maker's config, and its nodes' passphrases beside it,
 * into `dir`: the `targets` to trade through, its two nodes, each with its
 * hub's ledger and the owner book that funds it, and the pool on the fake
 * ledger. The XRP node's band is `xrpBand` around what it holds now, in
 * its hub and its book. Returns the config's path.
 */
export async function writeConfig(
  scenario: Scenario,
  dir: string,
  targets: string[],
  xrpNode: ScenarioNode,
  dsusdNode: ScenarioNode,
  xrpBand = XRP_BAND,
): Promise<string> {
  for (const node of [xrpNode, dsusdNode]) {
    const file = join(dir, `${node.name}.phrase`);
    await writeFile(file, `${node.identity.phrase}\n`, { mode: 0o600 });
  }
  const xrpHeld = await held(xrpNode, XRP);
  const config = {
    targets,
    xrpl: {
      server: scenario.ledger!.url,
      token: { currency: "DSUSD", issuer: ISSUER },
    },
    xrp: {
      socket: xrpNode.socket,
      ledger: XRP_HUB,
      ownerBook: XRP_BOOK,
      passphraseFile: join(dir, `${xrpNode.name}.phrase`),
      tradeSize: Number(TRADE),
      band: {
        low: Number(xrpHeld - xrpBand),
        high: Number(xrpHeld + xrpBand),
      },
    },
    token: {
      socket: dsusdNode.socket,
      ledger: DSUSD_HUB,
      ownerBook: DSUSD_BOOK,
      passphraseFile: join(dir, `${dsusdNode.name}.phrase`),
      tradeSize: Number(TRADE),
      band: { low: Number(DSUSD_LOW), high: Number(DSUSD_HIGH) },
    },
    marginPpm: MARGIN_PPM,
    slippagePpm: SLIPPAGE_PPM,
    caFile: scenario.caFile,
    allowPrivate: true,
  };
  const file = join(dir, "config.json");
  await writeFile(file, JSON.stringify(config));
  return file;
}
