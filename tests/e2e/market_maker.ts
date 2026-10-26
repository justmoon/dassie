/**
 * What the market maker's end-to-end tests share
 * (spec/areas/rate-feed.md#market-maker): reading the nodes' books, the
 * fake ledger's pool, and writing the market maker's config. A scenario
 * runs one or more core nodes with XRP and DSUSD, and the market maker's
 * two periphery nodes, one with each, pinned to every core node.
 *
 * @module
 */

import { assert } from "@std/assert";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Agent, fetch } from "undici";
import { LedgerConnection } from "@dassie/rpc/proto";
import type { Scenario, ScenarioNode } from "@dassie/dev/scenario";

export const XRP = "xrpl-testnet-xrp+xrp";
export const DSUSD = "xrpl-testnet-dsusd+usd";
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
/**
 * Longer than a node's settlement interval (1 s with dev timings): a
 * settlement that is due starts within it.
 */
const SETTLE_CHECK_MS = 5_000;
/** How long the fake ledger may take to answer `amm_info`. */
const AMM_INFO_TIMEOUT_MS = 10_000;

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

/** The XRP a node's account holds, in the books' units: for the DSUSD
 * node, which runs no XRP ledger, its gas, off its books. */
async function accountXrp(node: ScenarioNode): Promise<bigint> {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const holding = externalAssets
    .flatMap((asset) => asset.holdings)
    .find((holding) => holding.assetCode === "XRP");
  assert(holding, `${node.name}'s account shows no XRP`);
  return holding.amount * 10n ** BigInt(9 - holding.assetScale);
}

/** The node IDs `node` is peered with, sorted. */
export async function peersOf(node: ScenarioNode): Promise<string[]> {
  const { peers } = await node.rpc.call("listPeers", {});
  return peers.map((peer) => peer.node).sort();
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

/**
 * What each node owes each peer net, by its own books, by
 * `node>peer@ledger`: its peer account less the credit it extends,
 * negative when the peer owes it.
 */
export async function peerAccounts(
  scenario: Scenario,
): Promise<Map<string, bigint>> {
  const byId = new Map(scenario.nodes.map((node) => [node.id, node.name]));
  const accounts = new Map<string, bigint>();
  for (const node of scenario.nodes) {
    const list = await node.rpc.call("listAccounts", {});
    for (const account of list.accounts) {
      const match = /^(.+):(assets\/interledger|contra\/trust)\/(.+)$/
        .exec(account.path);
      if (!match) continue;
      const peer = byId.get(match[3]) ?? match[3];
      const key = `${node.name}>${peer}@${match[1]}`;
      const net = account.creditsPosted - account.debitsPosted;
      accounts.set(key, (accounts.get(key) ?? 0n) + net);
    }
  }
  return accounts;
}

/** What each node's books hold: per ledger, on the ledger and the
 * owner's. */
export async function books(
  scenario: Scenario,
): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  for (const node of scenario.nodes) {
    const { ledgers } = await node.rpc.call("listLedgers", {});
    for (const entry of ledgers) {
      out.set(
        `${node.name}@${entry.id}`,
        `${entry.onLedgerBalance}/${entry.ownerBalance}`,
      );
    }
  }
  return out;
}

/**
 * Settling went quiet: no payment or settlement on its way, every ledger
 * followed, each pair of books agrees, and there are at least `least` peer
 * accounts, both sides of each peering counted.
 */
export async function quiet(
  scenario: Scenario,
  least: number,
): Promise<boolean> {
  for (const node of scenario.nodes) {
    const { channels } = await node.rpc.call("listSettlementChannels", {});
    if (channels.some((channel) => channel.sending)) return false;
    const { payments } = await node.rpc.call("listOutgoingPayments", {});
    if (payments.some((payment) => payment.state === "sending")) return false;
    const { ledgers } = await node.rpc.call("listLedgers", {});
    if (ledgers.some((view) => view.connection !== LedgerConnection.SYNCED)) {
      return false;
    }
  }
  const accounts = await peerAccounts(scenario);
  for (const [key, mine] of accounts) {
    const [pair, on] = key.split("@");
    const [node, peer] = pair.split(">");
    const theirs = accounts.get(`${peer}>${node}@${on}`);
    if (theirs === undefined || mine + theirs !== 0n) return false;
  }
  return accounts.size >= least;
}

/** Settling went quiet and stays so: a settlement that is due but not
 * started yet shows only in a while, as the books move. */
export async function settled(
  scenario: Scenario,
  least: number,
): Promise<boolean> {
  if (!(await quiet(scenario, least))) return false;
  const before = await books(scenario);
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  const after = await books(scenario);
  return (await quiet(scenario, least)) &&
    [...before].every(([key, value]) => after.get(key) === value);
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

/** What the market maker holds, in the books' units: XRP on its XRP
 * node's books and its DSUSD node's gas, and DSUSD on its DSUSD node's
 * books. */
export interface Holdings {
  readonly xrp: bigint;
  readonly dsusd: bigint;
}

export async function holdings(
  xrpNode: ScenarioNode,
  dsusdNode: ScenarioNode,
): Promise<Holdings> {
  return {
    xrp: (await owner(xrpNode, XRP)) + (await accountXrp(dsusdNode)),
    dsusd: await owner(dsusdNode, DSUSD),
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
 * into `dir`: the `targets` to trade through, its two nodes, and the pool
 * on the fake ledger. The XRP node's band is `xrpBand` around what it holds
 * now. Returns the config's path.
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
  const xrpHeld = await owner(xrpNode, XRP);
  const config = {
    targets,
    xrpl: {
      server: scenario.ledger!.url,
      token: { currency: "DSUSD", issuer: ISSUER },
    },
    xrp: {
      socket: xrpNode.socket,
      ledger: XRP,
      passphraseFile: join(dir, `${xrpNode.name}.phrase`),
      tradeSize: Number(TRADE),
      band: {
        low: Number(xrpHeld - xrpBand),
        high: Number(xrpHeld + xrpBand),
      },
    },
    token: {
      socket: dsusdNode.socket,
      ledger: DSUSD,
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
