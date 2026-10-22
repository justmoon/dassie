/**
 * End to end (spec/areas/testing.md#end-to-end): passive rebalancing, the
 * whole loop (spec/decisions/passive-rebalancing.md). The `market-maker`
 * scenario runs a core node, d1, with XRP and DSUSD, most of its value in
 * XRP, and a market maker's two periphery nodes pinned to it: d2 with XRP,
 * d3 with DSUSD. The fake ledger holds an XRP/DSUSD pool at the dev rate.
 *
 * d1 lacks DSUSD, so its curve takes DSUSD in at better than spot
 * (spec/areas/connector.md#inventory-pricing), and its feed says so
 * (spec/areas/rate-feed.md). `dassie market-maker`, from the e2e build,
 * reads the feed, pays DSUSD from d3 through d1 to d2, which d1 converts
 * to XRP, and once d2 holds more XRP than its band, swaps it back into
 * DSUSD through the pool (spec/areas/rate-feed.md#market-maker). So d1's
 * share of its value in DSUSD rises, toward its target; the market
 * maker's value, at the pool's price, grows; and every node's books are
 * as they were after all three restart.
 */

import { test } from "node:test";
import { assert, assertEquals } from "@std/assert";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent, fetch } from "undici";
import { LedgerConnection } from "@dassie/rpc/proto";
import { ChildProcess } from "@dassie/dev/process";

import {
  DASSIE,
  FAKE_LEDGER,
  missingScenarioTools,
  SCENARIO_PORT_OFFSET,
} from "./harness.ts";
import {
  DEV_RATES,
  findScenario,
  poolDrops,
  Scenario,
  type ScenarioNode,
} from "@dassie/dev/scenario";

const XRP = "xrpl-testnet+xrp";
const DSUSD = "xrpl-testnet-dsusd+usd";
/** DSUSD's issuer on the testnet, which the fake ledger issues it at. */
const ISSUER = "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M";
/** DSUSD's currency code as the ledger holds it: its letters, padded. */
const DSUSD_CURRENCY = "4453555344000000000000000000000000000000";
/** A whole XRP or dollar in the books' units, which count at scale 9. */
const UNIT = 1_000_000_000n;
/** What the market maker sends through d1 in one trade: 2 dollars. */
const TRADE = 2n * UNIT;
/**
 * The least gain a trade must make: 1%. d1 holds about a twentieth of its
 * value in DSUSD, so its curve takes DSUSD in at up to its cap, 5% above
 * spot, less each packet's own impact: a trade clears the margin after the
 * pool's fee by a few percent.
 */
const MARGIN_PPM = 10_000;
/** How far the pool's price may move against a swap: 1%. */
const SLIPPAGE_PPM = 10_000;
/**
 * d2's band around what it holds at the start: above it, after two trades,
 * the market maker swaps its XRP back into DSUSD through the pool.
 */
const XRP_BAND = 6n * UNIT;
/** d3's band, in dollars: room for every trade the run makes. */
const DSUSD_LOW = 5n * UNIT;
const DSUSD_HIGH = 40n * UNIT;
/**
 * How long the market maker may take to trade and swap back: it decides
 * every 10 s, and a trade waits on settlement both ways through d1.
 */
const RUN_MS = 300_000;
/**
 * Longer than a node's settlement interval (1 s with dev timings): a
 * settlement that is due starts within it.
 */
const SETTLE_CHECK_MS = 5_000;
/** How long the fake ledger may take to answer `amm_info`. */
const AMM_INFO_TIMEOUT_MS = 10_000;
const PORT_OFFSET = SCENARIO_PORT_OFFSET + 100;

const missing = await missingScenarioTools();

/** A node's ledger, as its owner sees it. */
async function ledger(node: ScenarioNode, id: string) {
  const { ledgers } = await node.rpc.call("listLedgers", {});
  return ledgers.find((entry) => entry.id === id);
}

/** What the owner holds on a node's ledger, by its books. */
async function owner(node: ScenarioNode, id: string): Promise<bigint> {
  const found = await ledger(node, id);
  assert(found, `${node.name} has no ${id}`);
  return found.ownerBalance;
}

/** The XRP a node's account holds, in the books' units: for d3, which runs
 * no XRP ledger, its gas, off its books. */
async function accountXrp(node: ScenarioNode): Promise<bigint> {
  const { externalAssets } = await node.rpc.call("listLedgers", {});
  const holding = externalAssets
    .flatMap((asset) => asset.holdings)
    .find((holding) => holding.assetCode === "XRP");
  assert(holding, `${node.name}'s account shows no XRP`);
  return holding.amount * 10n ** BigInt(9 - holding.assetScale);
}

/** Every node follows the ledger and is funded, and d1 has fresh rates. */
async function funded(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3] = scenario.nodes;
  const minted = BigInt(findScenario("market-maker")!.mint!.value) * UNIT;
  const views: [ScenarioNode, string, bigint][] = [
    [d1, XRP, 1n],
    [d1, DSUSD, minted],
    [d2, XRP, 1n],
    [d3, DSUSD, minted],
  ];
  for (const [node, id, least] of views) {
    const view = await ledger(node, id);
    if (view?.connection !== LedgerConnection.SYNCED) return false;
    if (view.ownerBalance < least) return false;
  }
  const { rates } = await d1.rpc.call("listLedgers", {});
  return ["XRP", "USD"].every((code) =>
    rates.some((rate) => rate.assetCode === code && !rate.stale)
  );
}

/** d2 and d3 are peered with d1, their only peer. */
async function peered(scenario: Scenario): Promise<boolean> {
  const [d1, d2, d3] = scenario.nodes;
  const peers = async (node: ScenarioNode) =>
    (await node.rpc.call("listPeers", {})).peers.map((peer) => peer.node)
      .sort();
  const ofD1 = await peers(d1);
  return ofD1.length === 2 && ofD1.includes(d2.id) && ofD1.includes(d3.id) &&
    (await peers(d2)).join() === d1.id && (await peers(d3)).join() === d1.id;
}

/** The port d1's `GET /stats` names for its feed, if it names one. */
async function feedPort(scenario: Scenario): Promise<number | undefined> {
  const [d1] = scenario.nodes;
  const agent = new Agent({
    connect: { ca: await readFile(scenario.caFile, "utf8") },
  });
  try {
    const response = await fetch(`${d1.localUrl}/stats`, {
      dispatcher: agent,
    });
    const stats = await response.json() as { node?: { feedPort?: number } };
    return stats.node?.feedPort;
  } finally {
    await agent.close();
  }
}

/**
 * What each node owes each peer net, by its own books, by
 * `node>peer@ledger`: its peer account less the credit it extends,
 * negative when the peer owes it.
 */
async function peerAccounts(scenario: Scenario): Promise<Map<string, bigint>> {
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
async function books(scenario: Scenario): Promise<Map<string, string>> {
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
 * followed, and each pair of books agrees.
 */
async function quiet(scenario: Scenario): Promise<boolean> {
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
  return accounts.size === 4;
}

/** Settling went quiet and stays so: a settlement that is due but not
 * started yet shows only in a while, as the books move. */
async function settled(scenario: Scenario): Promise<boolean> {
  if (!(await quiet(scenario))) return false;
  const before = await books(scenario);
  await new Promise((resolve) => setTimeout(resolve, SETTLE_CHECK_MS));
  const after = await books(scenario);
  return (await quiet(scenario)) &&
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
interface Pool {
  readonly xrp: bigint;
  readonly dsusd: bigint;
}

/** Reads the pool with `amm_info` from the fake ledger. */
async function pool(scenario: Scenario): Promise<Pool> {
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

/** What the market maker holds, in the books' units: XRP on d2's books
 * and d3's gas, and DSUSD on d3's books. */
interface Holdings {
  readonly xrp: bigint;
  readonly dsusd: bigint;
}

async function holdings(scenario: Scenario): Promise<Holdings> {
  const [, d2, d3] = scenario.nodes;
  return {
    xrp: (await owner(d2, XRP)) + (await accountXrp(d3)),
    dsusd: await owner(d3, DSUSD),
  };
}

/** `held`'s value in dollars at `pool`'s price, times what the pool holds
 * of XRP: exact, and as good to compare at one price. */
function value(held: Holdings, at: Pool): bigint {
  return held.dsusd * at.xrp + held.xrp * at.dsusd;
}

/** A value from {@linkcode value}, in dollars, for messages. */
function dollars(scaled: bigint, at: Pool): string {
  const micros = scaled * 1_000_000n / at.xrp / UNIT;
  return `${micros / 1_000_000n}.${`${micros % 1_000_000n}`.padStart(6, "0")}`;
}

/** The market maker's config: d1 as the target, d2 and d3 as its nodes,
 * and the pool on the fake ledger. */
function config(
  scenario: Scenario,
  dir: string,
  xrpHeld: bigint,
): Record<string, unknown> {
  const [d1, d2, d3] = scenario.nodes;
  return {
    target: { url: d1.url },
    xrpl: {
      server: scenario.ledger!.url,
      token: { currency: "DSUSD", issuer: ISSUER },
    },
    xrp: {
      socket: d2.socket,
      ledger: XRP,
      passphraseFile: join(dir, "d2.phrase"),
      tradeSize: Number(TRADE),
      band: {
        low: Number(xrpHeld - XRP_BAND),
        high: Number(xrpHeld + XRP_BAND),
      },
    },
    token: {
      socket: d3.socket,
      ledger: DSUSD,
      passphraseFile: join(dir, "d3.phrase"),
      tradeSize: Number(TRADE),
      band: { low: Number(DSUSD_LOW), high: Number(DSUSD_HIGH) },
    },
    marginPpm: MARGIN_PPM,
    slippagePpm: SLIPPAGE_PPM,
    caFile: scenario.caFile,
    allowPrivate: true,
  };
}

test("a market maker moves a skewed core node toward balance, and gains", {
  skip: missing !== undefined,
}, async () => {
  const scenario = await Scenario.temporary("market-maker", {
    dassie: DASSIE,
    fakeLedger: FAKE_LEDGER,
    portOffset: PORT_OFFSET,
  });
  const dir = await mkdtemp(join(tmpdir(), "dassie-market-maker-"));
  let maker: ChildProcess | undefined;
  try {
    const [d1, d2, d3] = scenario.nodes;
    await scenario.until("funded", () => funded(scenario), 120_000);
    await scenario.until("d2 and d3 peered with d1", () => peered(scenario));
    await scenario.until(
      "d1 names its feed",
      async () => (await feedPort(scenario)) === d1.feedPort,
    );
    // d3 pays d1 in advance, which extends it nothing until then
    // (spec/decisions/earned-trust.md).
    await scenario.until("paid in advance", () => settled(scenario), 120_000);

    const seeded = await pool(scenario);
    const def = findScenario("market-maker")!;
    assertEquals(
      seeded.xrp,
      poolDrops(def.pool!.value, DEV_RATES.XRP) * 1_000n,
      "the pool at the dev rate",
    );
    const d1Xrp = await owner(d1, XRP);
    const d1Dsusd = await owner(d1, DSUSD);
    const before = await holdings(scenario);

    for (const node of [d2, d3]) {
      const file = join(dir, `${node.name}.phrase`);
      await writeFile(file, `${node.identity.phrase}\n`, { mode: 0o600 });
    }
    const file = join(dir, "config.json");
    await writeFile(
      file,
      JSON.stringify(config(scenario, dir, await owner(d2, XRP))),
    );
    maker = new ChildProcess("market-maker", DASSIE, {
      args: ["market-maker", "--config", file],
      env: { DASSIE_LOG_LEVEL: "info" },
    });
    let exited = false;
    maker.exited.then(() => exited = true);
    try {
      // A trade's DSUSD reached d1, and d2's XRP went back through the
      // pool: the only swaps on it are the market maker's.
      await scenario.until(
        "the market maker trades through d1 and swaps back through the pool",
        async () => {
          if (exited) throw new Error("the market maker exited");
          const now = await pool(scenario);
          return now.xrp > seeded.xrp &&
            (await owner(d1, DSUSD)) >= d1Dsusd + TRADE;
        },
        RUN_MS,
      );
    } catch (error) {
      const { output } = await maker.stop();
      maker = undefined;
      throw new Error(`${(error as Error).message}\n${output}`);
    }
    const { code, output } = await maker.stop();
    maker = undefined;
    // SIGTERM: it stops at once, as asked.
    assertEquals(code, 0, output);
    // What it paid is still on its way: the nodes finish it.
    await scenario.until("settled", () => settled(scenario), 180_000);

    // d1 holds a larger share of its value in DSUSD, the asset it lacked:
    // DSUSD over XRP rose, which at one rate is its share.
    const d1XrpAfter = await owner(d1, XRP);
    const d1DsusdAfter = await owner(d1, DSUSD);
    assert(
      d1DsusdAfter * d1Xrp > d1Dsusd * d1XrpAfter,
      `d1 went from ${d1Dsusd} DSUSD and ${d1Xrp} XRP ` +
        `to ${d1DsusdAfter} DSUSD and ${d1XrpAfter} XRP`,
    );
    assert(d1DsusdAfter > d1Dsusd, "d1 took DSUSD in");
    assert(d1XrpAfter < d1Xrp, "d1 paid XRP out");

    // The market maker holds more than it did, at the pool's price now.
    const after = await holdings(scenario);
    const now = await pool(scenario);
    assert(now.xrp > seeded.xrp && now.dsusd < seeded.dsusd, "it swapped");
    const [was, is] = [value(before, now), value(after, now)];
    assert(
      is > was,
      `the market maker's value went from ${dollars(was, now)} dollars ` +
        `to ${dollars(is, now)}`,
    );

    // Restarted, all three catch up and book nothing twice.
    const kept = await books(scenario);
    const accounts = await peerAccounts(scenario);
    for (const node of scenario.nodes) {
      const { code, output } = (await node.stop())!;
      assertEquals(code, 0, output);
    }
    for (const node of scenario.nodes) {
      await node.start();
      await node.waitForEngine();
    }
    await scenario.until("caught up", () => quiet(scenario), 120_000);
    assertEquals(await books(scenario), kept, "the books after a restart");
    assertEquals(await peerAccounts(scenario), accounts, "peer accounts");
  } finally {
    await maker?.stop();
    await rm(dir, { recursive: true, force: true });
    const failures = await scenario.stop();
    assertEquals(failures, [], failures.join("\n"));
  }
});
