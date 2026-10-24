/**
 * The scenarios (spec/areas/dev.md#scenarios) as real `dassie` binaries on this
 * machine, set up through their own API. The dev environment runs them, and so
 * do the end-to-end tests, in a directory of their own
 * ({@linkcode Scenario.temporary}).
 *
 * @module
 */

import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { caFile, certificate } from "../certs.ts";
import {
  askIssuer,
  type FakeLedger,
  type IssuerRequest,
  startFakeLedger,
  xrplConfig,
} from "../fake_ledger.ts";
import { type Layout, layout as defaultLayout } from "../paths.ts";
import { DEV_RATES, type RateServer, startRates } from "../rates.ts";
import type { Stream } from "../process.ts";
import { identity, loadTable, type Table } from "../vanity.ts";
import { until } from "../wait.ts";
import { type NodeEnvironment, ScenarioNode } from "./node.ts";
import {
  findScenario,
  invalid,
  type IssuedToken,
  MAX_NODES,
  minted,
  peripheryOf,
  poolDrops,
  type ScenarioDef,
  settlesOnXrpl,
  unavailable,
  XRPL_SCHEMES,
} from "./scenarios.ts";

export { ScenarioNode } from "./node.ts";
export {
  DEFAULT_SCENARIO,
  findScenario,
  type IssuedToken,
  MAX_NODES,
  type Periphery,
  type PoolDef,
  poolDrops,
  type ScenarioDef,
  SCENARIOS,
  TOKEN_SCHEMES,
  unavailable,
  XRPL_SCHEMES,
} from "./scenarios.ts";
export { type Layout, layout, socketPath } from "../paths.ts";
export { caFile, certificate } from "../certs.ts";
export { fakeLedgerBinary, type IssuerRequest } from "../fake_ledger.ts";
export { DEV_RATES, RATES_PORT, type RateTable } from "../rates.ts";
export { until, waitForPort, waitForSocket } from "../wait.ts";
export type { Exit } from "../process.ts";

/**
 * How long a node may take to show its token's line: the faucet funds its
 * account in the next ledger, and its `TrustSet` applies in the one after.
 */
const LINE_WITHIN_MS = 60_000;

/** How a scenario runs. */
export interface ScenarioOptions {
  /** The `dassie` binary. */
  readonly dassie: string;
  readonly layout: Layout;
  /** Added to every port, so that environments can run side by side. */
  readonly portOffset?: number;
  /** More environment for every node. */
  readonly env?: Readonly<Record<string, string>>;
  /** Nodes left without a settlement scheme, by index. */
  readonly withoutLedger?: readonly number[];
  /**
   * The fake XRP Ledger's binary: a scenario that settles on the XRP
   * Ledger runs one and settles on it, rather than on the public testnet.
   */
  readonly fakeLedger?: string;
  /** Each line a node prints. */
  readonly onLine?: (node: string, stream: Stream, line: string) => void;
  /** Each node's process, as soon as it is spawned. */
  readonly onSpawn?: (node: ScenarioNode) => void;
  /**
   * Each node's progress: started, then set up; or failed, with why and
   * the end of its output.
   */
  readonly onNode?: (
    node: ScenarioNode,
    phase: NodePhase,
    error?: string,
  ) => void;
}

/** How far a node got. */
export type NodePhase = "starting" | "setting-up" | "running" | "failed";

/** A running scenario. */
export class Scenario {
  readonly nodes: ScenarioNode[] = [];
  #table: Table;
  #environment: NodeEnvironment;
  #temporary: boolean;
  /** The fake XRP Ledger, if the scenario runs one. */
  ledger?: FakeLedger;
  /** The exchange rates every node fetches. */
  rates?: RateServer;
  /** What each scheme is set up with, by ID. */
  #configs: Record<string, string> = {};
  readonly def: ScenarioDef;
  readonly options: ScenarioOptions;

  private constructor(
    def: ScenarioDef,
    options: ScenarioOptions,
    environment: NodeEnvironment,
    temporary: boolean,
  ) {
    this.def = def;
    this.options = options;
    this.#table = loadTable();
    this.#environment = environment;
    this.#temporary = temporary;
  }

  /**
   * Starts scenario `id` and sets each node up. Data left in the layout by
   * an earlier run of the same scenario is kept: each node resumes.
   */
  static async start(id: string, options: ScenarioOptions): Promise<Scenario> {
    return await Scenario.#start(id, options, false);
  }

  /** {@linkcode start} in a new directory, deleted when it stops. */
  static async temporary(
    id: string,
    options: Omit<ScenarioOptions, "layout">,
  ): Promise<Scenario> {
    const dir = await mkdtemp(join(tmpdir(), `dassie-${id}-`));
    return await Scenario.#start(
      id,
      // A CA of its own: the developer's stays untouched.
      { ...options, layout: await defaultLayout(dir, `${dir}/ca`) },
      true,
    );
  }

  static async #start(
    id: string,
    options: ScenarioOptions,
    temporary: boolean,
  ): Promise<Scenario> {
    const def = findScenario(id);
    if (!def) throw new Error(`no scenario ${id}`);
    const why = unavailable(def, options.fakeLedger !== undefined) ??
      invalid(def);
    if (why) throw new Error(`${id}: ${why}`);
    const ca = await caFile(options.layout.tls, options.layout.caRoot);
    const environment: NodeEnvironment = {
      dassie: options.dassie,
      layout: options.layout,
      caFile: ca,
      portOffset: options.portOffset ?? 0,
      env: options.env ?? {},
      onLine: options.onLine,
      onSpawn: options.onSpawn,
    };
    const scenario = new Scenario(def, options, environment, temporary);
    try {
      // Every node's rates come from here, under the scenario's CA.
      const rates = await certificate(options.layout.tls, "rates", [
        "localhost",
        "127.0.0.1",
      ], options.layout.caRoot);
      scenario.rates = await startRates(rates, options.portOffset ?? 0);
      if (settlesOnXrpl(def) && options.fakeLedger) {
        // Its faucet is at `localhost`, under the scenario's CA, which the
        // nodes trust.
        const faucet = await certificate(options.layout.tls, "fake-ledger", [
          "localhost",
          "127.0.0.1",
        ], options.layout.caRoot);
        scenario.ledger = await startFakeLedger(
          options.fakeLedger,
          faucet,
          options.portOffset ?? 0,
        );
        // Every XRPL scheme settles through the node's one account, which
        // they set up alike.
        for (const scheme of XRPL_SCHEMES) {
          scenario.#configs[scheme] = xrplConfig(scenario.ledger);
        }
        const pool = def.pool;
        if (pool !== undefined) {
          // At the rates the nodes start with, so that the pool and the
          // nodes agree on the price until something trades.
          await scenario.askIssuer({
            action: "amm",
            token: pool.token,
            drops: `${poolDrops(pool.value, DEV_RATES.XRP)}`,
            value: pool.value,
            fee: pool.tradingFee,
          });
        }
      }
      for (let index = 0; index < def.schemes.length; index++) {
        await scenario.#startNode(index);
      }
      const mint = def.mint;
      if (mint !== undefined) {
        await Promise.all(
          scenario.nodes
            .filter((node) => scenario.#minted(node.index))
            .map((node) => scenario.mintToken(node, mint.token, mint.value)),
        );
      }
    } catch (error) {
      await scenario.stop();
      throw error;
    }
    return scenario;
  }

  /** The CA the nodes' certificates come from. */
  get caFile(): string {
    return this.#environment.caFile;
  }

  /** The bootstrap nodes, as `DASSIE_BOOTSTRAP_NODES` names them. */
  #bootstrapNodes(): string {
    return JSON.stringify(
      this.def.bootstrap.map((index) => {
        const node = new ScenarioNode(
          identity(this.#table, index),
          this.#environment,
        );
        return node.bootstrapEntry();
      }),
    );
  }

  /** The schemes node `index` runs: the scenario's, or an added node's. */
  #schemes(index: number): readonly string[] {
    if (this.options.withoutLedger?.includes(index)) return [];
    return this.def.schemes[index] ?? this.def.addedSchemes;
  }

  /** Whether node `index` is minted the scenario's token. */
  #minted(index: number): boolean {
    return minted(this.def, this.#schemes(index));
  }

  /**
   * The environment of node `index`: the bootstrap nodes, and for a
   * periphery node the IDs of the nodes it is pinned to.
   */
  #nodeEnv(index: number): Record<string, string> {
    const env: Record<string, string> = {
      DASSIE_BOOTSTRAP_NODES: this.#bootstrapNodes(),
    };
    const periphery = peripheryOf(this.def, index);
    if (periphery !== undefined) {
      env.DASSIE_UPLINKS = periphery.uplinks
        .map((uplink) => identity(this.#table, uplink).id)
        .join(",");
    }
    return env;
  }

  async #startNode(index: number): Promise<ScenarioNode> {
    if (index >= MAX_NODES) throw new Error(`at most ${MAX_NODES} nodes`);
    const node = new ScenarioNode(
      identity(this.#table, index),
      this.#environment,
      this.#nodeEnv(index),
      peripheryOf(this.def, index) !== undefined,
    );
    this.nodes.push(node);
    const report = this.options.onNode ?? (() => {});
    try {
      report(node, "starting");
      await node.start();
      report(node, "setting-up");
      await node.setUp(this.#schemes(index), this.#configs, this.rates?.url);
    } catch (error) {
      // A node that failed is stopped and dropped, so its index is free.
      const exit = await node.stop();
      this.nodes.splice(this.nodes.indexOf(node), 1);
      const output = exit?.output ? `\n${exit.output}` : "";
      report(node, "failed", `${(error as Error).message}${output}`);
      throw error;
    }
    report(node, "running");
    return node;
  }

  /** Starts one more node, with the scenario's schemes for added nodes. */
  async addNode(): Promise<ScenarioNode> {
    const node = await this.#startNode(this.nodes.length);
    if (this.def.mint !== undefined && this.#minted(node.index)) {
      await this.mintToken(node, this.def.mint.token, this.def.mint.value);
    }
    return node;
  }

  /** Asks the fake ledger's issuer of test DSUSD, or of the request's
   * token, to act. */
  async askIssuer(request: IssuerRequest): Promise<void> {
    if (!this.ledger) throw new Error("the scenario runs no fake ledger");
    await askIssuer(this.ledger, this.caFile, request);
  }

  /**
   * Has the fake ledger's issuer of `token` mint `value` of it, in dollars,
   * to `node`'s account, once the node shows its trust line: the issuer
   * can pay only over a line, which the node's token scheme sets up
   * itself.
   */
  async mintToken(
    node: ScenarioNode,
    token: IssuedToken,
    value: string,
  ): Promise<void> {
    let address = "";
    await until(`${node.name}'s ${token} line`, async () => {
      const { externalAssets } = await node.rpc.call("listLedgers", {});
      const account = externalAssets.find((asset) =>
        asset.holdings.some((holding) => holding.assetCode === token)
      );
      address = account?.address ?? "";
      return address !== "";
    }, LINE_WITHIN_MS);
    await this.askIssuer({
      action: "mint",
      token,
      destination: address,
      value,
    });
  }

  /**
   * Stops every node, all at once, so stopping takes as long as the
   * slowest; the output of those that did not exit cleanly. A temporary
   * scenario's files are deleted.
   */
  async stop(): Promise<string[]> {
    const exits = await Promise.all(this.nodes.map((node) => node.stop()));
    await this.ledger?.stop();
    await this.rates?.stop();
    const failures: string[] = [];
    this.nodes.forEach((node, index) => {
      const exit = exits[index];
      if (exit && exit.code !== 0) {
        failures.push(`${node.name} exited ${exit.code}:\n${exit.output}`);
      }
    });
    if (this.#temporary) {
      const { state, sockets } = this.options.layout;
      await rm(state, { recursive: true, force: true }).catch(() => {});
      await rm(sockets, { recursive: true, force: true }).catch(() => {});
    }
    return failures;
  }

  /** Waits until `done` holds, polling. */
  until(
    what: string,
    done: () => Promise<boolean>,
    timeoutMs?: number,
  ): Promise<void> {
    return until(what, done, timeoutMs);
  }

  /**
   * Whether the network has converged: every node knows every other's link
   * state, has a route to each, and as many peers as it looks for. Not for
   * a scenario with periphery nodes, to which no other node routes.
   */
  async converged(): Promise<boolean> {
    const count = this.nodes.length;
    for (const node of this.nodes) {
      const { routes } = await node.rpc.call("listRoutes", {});
      const { peers } = await node.rpc.call("listPeers", {});
      if (routes.length < count - 1) return false;
      if (peers.length < Math.min(2, count - 1)) return false;
    }
    return true;
  }

  /** Each node's peers, by node name. */
  async peerings(): Promise<Map<string, string[]>> {
    const byId = new Map(this.nodes.map((node) => [node.id, node.name]));
    const peerings = new Map<string, string[]>();
    for (const node of this.nodes) {
      const { peers } = await node.rpc.call("listPeers", {});
      peerings.set(
        node.name,
        peers.map((peer) => byId.get(peer.node) ?? peer.node).sort(),
      );
    }
    return peerings;
  }
}
