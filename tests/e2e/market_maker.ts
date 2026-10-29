/**
 * What the end-to-end tests of swaps and the market maker share
 * (spec/areas/rate-feed.md#market-maker, spec/decisions/peer-swaps.md):
 * the scenario's nodes, reading what they hold across their hubs and
 * owner books, the peers' snapshots a node holds, writing the market
 * maker's config, and running `dassie market-maker` until it trades. A
 * scenario founds an XRP hub and a DSUSD hub; its core nodes, the
 * targets, join both, with XRP and DSUSD, and so does the market maker's
 * one periphery node, pinned to every core node.
 *
 * @module
 */

import { assert, assertEquals } from "@std/assert";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import type { PeerSnapshot } from "@dassie/rpc/proto";
import { ChildProcess } from "@dassie/dev/process";
import {
  hubLedger,
  hubOwnerLedger,
  hubsOf,
  joins,
  type Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

import { DASSIE } from "./harness.ts";

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
/** A whole XRP or dollar in the books' units, which count at scale 9. */
export const UNIT = 1_000_000_000n;
/** The most the market maker pays through a core node in one trade, of
 * either asset: 2 units. */
export const CAP = 2n * UNIT;
/**
 * Its curve, 1%, the least gain a trade must make after the cost of
 * undoing it, 0.2%, how far an asset's share may be from even before it
 * rebalances, 5 points, and how far the pool's price may move against a
 * rebalance, 1% (spec/decisions/market-maker-inventory.md). Its node starts
 * with the faucet's XRP and a little DSUSD, so it rebalances to even
 * first, through the pool, and then trades.
 */
const CURVE_PPM = 10_000;
const MARGIN_PPM = 2_000;
const THRESHOLD_PPM = 50_000;
const SLIPPAGE_PPM = 10_000;
/**
 * How long the market maker may take to make its first trade: it decides
 * as it starts, rebalances first and waits for its node to book that, and
 * the swap goes as fast as the node settles over both hubs. A hang's guard,
 * not what the test decides by.
 */
const TRADE_WITHIN_MS = 240_000;

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

/** What the owner holds on a node's ledger, by its books. */
async function owner(node: ScenarioNode, id: string): Promise<bigint> {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  const found = ledgers.find((entry) => entry.id === id);
  assert(found, `${node.name} has no ${id}`);
  return found.ownerBalance;
}

/**
 * What the owner holds of `asset` on a node: in its hub and in the owner
 * book that funds it, between which the node moves funds by itself
 * (spec/areas/hubs.md#in-the-node). A move on its way reads short.
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

/** Waits until each of `cores` has fresh rates and `makerNode` holds its
 * snapshot, which prices both hubs. */
export async function quoted(
  scenario: Scenario,
  cores: ScenarioNode[],
  makerNode: ScenarioNode,
): Promise<void> {
  for (const core of cores) {
    await scenario.until(
      `${core.name} has fresh rates`,
      () => freshRates(core),
    );
    await scenario.until(
      `the market maker's node holds ${core.name}'s snapshot`,
      async () => (await peerSnapshot(makerNode, core)) !== undefined,
    );
  }
}

/**
 * Writes the market maker's config, and its node's passphrase beside it,
 * into `dir`: its node, with each asset's hub ledger and the owner book
 * that funds it, and the pool on the fake ledger. Its targets are its
 * node's peers. Returns the config's path.
 */
export async function writeConfig(
  scenario: Scenario,
  dir: string,
  node: ScenarioNode,
): Promise<string> {
  const phrase = join(dir, `${node.name}.phrase`);
  await writeFile(phrase, `${node.identity.phrase}\n`, { mode: 0o600 });
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
      cap: Number(CAP),
    },
    token: {
      ledger: DSUSD_HUB,
      ownerBook: DSUSD_BOOK,
      cap: Number(CAP),
    },
    curvePpm: CURVE_PPM,
    marginPpm: MARGIN_PPM,
    thresholdPpm: THRESHOLD_PPM,
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

/**
 * Runs `dassie market-maker` from the e2e build with the config `file`
 * until its node's swaps for its owner, which only the market maker sends,
 * have one packet fulfilled: it read its node over the IPC socket, decided,
 * and traded. Then SIGTERM stops it at once, with code 0.
 */
export async function tradesOnce(
  scenario: Scenario,
  file: string,
  makerNode: ScenarioNode,
): Promise<void> {
  const maker = new ChildProcess("market-maker", DASSIE, {
    args: ["market-maker", "--config", file],
    env: { DASSIE_LOG: "info" },
  });
  let exited = false;
  maker.exited.then(() => exited = true);
  try {
    await scenario.until("the market maker trades", async () => {
      if (exited) throw new Error("the market maker exited");
      const status = await makerNode.rpc.call("getSwapStatus", {});
      return status.fulfilled > 0n;
    }, TRADE_WITHIN_MS);
  } catch (error) {
    const { output } = await maker.stop();
    throw new Error(`${(error as Error).message}\n${output}`);
  }
  const { code, output } = await maker.stop();
  assertEquals(code, 0, output);
}
