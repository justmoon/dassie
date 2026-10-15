/**
 * The scenarios of spec 6 §6.8 as real `dassie` binaries on this machine,
 * set up through their own API. The dev environment runs them, and so do
 * the end-to-end tests, in a directory of their own
 * ({@linkcode Scenario.temporary}).
 *
 * @module
 */

import { caFile, certificate } from "../certs.ts";
import {
  type FakeLedger,
  startFakeLedger,
  xrplConfig,
} from "../fake_ledger.ts";
import { type Layout, layout as defaultLayout } from "../paths.ts";
import { type RateServer, startRates } from "../rates.ts";
import type { Stream } from "../process.ts";
import { identity, loadTable, type Table } from "../vanity.ts";
import { until } from "../wait.ts";
import { type NodeEnvironment, ScenarioNode } from "./node.ts";
import { findScenario, MAX_NODES, type ScenarioDef } from "./scenarios.ts";

export { ScenarioNode } from "./node.ts";
export {
  DEFAULT_SCENARIO,
  findScenario,
  MAX_NODES,
  type ScenarioDef,
  SCENARIOS,
} from "./scenarios.ts";
export { type Layout, layout, socketPath } from "../paths.ts";
export { caFile, certificate } from "../certs.ts";
export { fakeLedgerBinary } from "../fake_ledger.ts";
export { DEV_RATES, RATES_PORT, type RateTable } from "../rates.ts";
export { until, waitForPort, waitForSocket } from "../wait.ts";
export type { Exit } from "../process.ts";

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

  private constructor(
    readonly def: ScenarioDef,
    readonly options: ScenarioOptions,
    environment: NodeEnvironment,
    temporary: boolean,
  ) {
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
    const dir = await Deno.makeTempDir({ prefix: `dassie-${id}-` });
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
    if (def.unavailable) throw new Error(`${id} ${def.unavailable}`);
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
      scenario.rates = startRates(rates, options.portOffset ?? 0);
      const xrpl = [...def.schemes, def.addedSchemes]
        .some((schemes) => schemes.includes("xrpl-testnet"));
      if (xrpl && options.fakeLedger) {
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
        scenario.#configs["xrpl-testnet"] = xrplConfig(scenario.ledger);
      }
      for (let index = 0; index < def.schemes.length; index++) {
        await scenario.#startNode(index, def.schemes[index]);
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

  async #startNode(
    index: number,
    schemes: readonly string[],
  ): Promise<ScenarioNode> {
    if (index >= MAX_NODES) throw new Error(`at most ${MAX_NODES} nodes`);
    const node = new ScenarioNode(
      identity(this.#table, index),
      this.#environment,
      { DASSIE_BOOTSTRAP_NODES: this.#bootstrapNodes() },
    );
    this.nodes.push(node);
    const report = this.options.onNode ?? (() => {});
    try {
      report(node, "starting");
      await node.start();
      report(node, "setting-up");
      const withoutLedger = this.options.withoutLedger?.includes(index);
      await node.setUp(
        withoutLedger ? [] : schemes,
        this.#configs,
        this.rates?.url,
      );
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
    return await this.#startNode(this.nodes.length, this.def.addedSchemes);
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
      await Deno.remove(state, { recursive: true }).catch(() => {});
      await Deno.remove(sockets, { recursive: true }).catch(() => {});
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
   * state, has a route to each, and as many peers as it looks for.
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
