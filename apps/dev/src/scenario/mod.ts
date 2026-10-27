/**
 * The scenarios (spec/areas/dev.md#scenarios) as real `dassie` binaries on this
 * machine, set up through their own API. The dev environment runs them, and so
 * do the end-to-end tests, in a directory of their own
 * ({@linkcode Scenario.temporary}).
 *
 * @module
 */

import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HubFoundingStage, LedgerConnection } from "@dassie/rpc/proto";
import { caFile, certificate } from "../certs.ts";
import {
  askIssuer,
  depositXrp,
  type FakeLedger,
  type IssuerRequest,
  startFakeLedger,
  xrplConfig,
} from "../fake_ledger.ts";
import { dataDir, type Layout, layout as defaultLayout } from "../paths.ts";
import { DEV_RATES, type RateServer, startRates } from "../rates.ts";
import type { Stream } from "../process.ts";
import { identity, loadTable, type Realm, type Table } from "../vanity.ts";
import { until } from "../wait.ts";
import { type NodeEnvironment, ScenarioNode } from "./node.ts";
import {
  hubScheme,
  hubSchemes,
  hubsOf,
  hubTerms,
  invalidHubs,
  joins,
  type ScenarioHub,
  sponsors,
  validatesAny,
} from "./hubs.ts";
import {
  findScenario,
  HUB_OWNER_BOOK,
  hubConfig,
  invalid,
  type IssuedToken,
  ledgerNetwork,
  live,
  MAINNET_DEPOSIT_DROPS,
  MAX_NODES,
  minted,
  peripheryOf,
  poolDrops,
  type ScenarioDef,
  settlesOnXrpl,
  unavailable,
  validatorFile,
  XRPL_SCHEMES,
} from "./scenarios.ts";

export { ScenarioNode } from "./node.ts";
export {
  hubLedger,
  hubOwnerBook,
  hubOwnerLedger,
  hubScheme,
  hubsOf,
  joins,
  type ScenarioHub,
  TOKEN_HUBS,
} from "./hubs.ts";
export {
  DEFAULT_SCENARIO,
  findScenario,
  HUB_LEDGER,
  HUB_OWNER_BOOK,
  HUB_SCHEME,
  type HubDef,
  type IssuedToken,
  joinsHub,
  live,
  MAINNET_DEPOSIT_DROPS,
  MAX_NODES,
  type Periphery,
  type PoolDef,
  poolDrops,
  resumes,
  type ScenarioDef,
  SCENARIOS,
  startsFresh,
  TOKEN_SCHEMES,
  unavailable,
  validates,
  XRPL_SCHEMES,
} from "./scenarios.ts";
export { type Layout, layout, socketPath } from "../paths.ts";
export { caFile, certificate } from "../certs.ts";
export {
  fakeLedgerBinary,
  type IssuerRequest,
  type XrplNetwork,
} from "../fake_ledger.ts";
export { DEV_RATES, RATES_PORT, type RateTable } from "../rates.ts";
export { until, waitForPort, waitForSocket } from "../wait.ts";
export type { Exit } from "../process.ts";

/**
 * How long a node may take to show its token's line: the faucet funds its
 * account in the next ledger, and its `TrustSet` applies in the one after.
 */
const LINE_WITHIN_MS = 60_000;

/** How long a node may take to show its mainnet account's address, once
 * its scheme is added. */
const ADDRESS_WITHIN_MS = 30_000;

/** How long a hub's sponsor may take to show XRP in its owner book: the
 * faucet funds its account in the next ledger. */
const SPONSOR_FUNDED_WITHIN_MS = 60_000;

/**
 * How long a hub may take to be founded, from the sponsor's request: each
 * validator accepts, signs and runs it within seconds, the escrow's
 * funding and setup take a ledger each, and the sponsor asks a validator
 * that did not answer again after a wait that doubles up to a minute
 * (spec/areas/hubs.md#founding-a-hub).
 */
const FOUNDED_WITHIN_MS = 180_000;

/** Where a validator's file is, in its data directory. */
const VALIDATOR_FILE = "validator.json";

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
  /** What each external asset is set up with, by ID. */
  #assets: Record<string, string> = {};
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
    this.#configs = { ...def.configs };
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
      invalid(def) ?? invalidHubs(def);
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
          undefined,
          ledgerNetwork(def),
        );
        // Every XRPL scheme settles through the node's one account on the
        // network, whose config is its own.
        const account = ledgerNetwork(def) === "mainnet"
          ? "xrpl-account/0"
          : "xrpl-testnet-account/0";
        scenario.#assets[account] = xrplConfig(scenario.ledger);
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
      await scenario.#foundHubs();
      // Mainnet has no faucet: each account is funded as an owner funds
      // it, before its token's line can exist.
      await Promise.all(scenario.nodes.map((node) => scenario.#fund(node)));
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

  /**
   * The realm the scenario's nodes run in, which their phrases name: a
   * secret of the table is another node in each realm.
   */
  get #realm(): Realm {
    return this.def.realm ?? "test";
  }

  /** The bootstrap nodes, as `DASSIE_BOOTSTRAP_NODES` names them. */
  #bootstrapNodes(): string {
    return JSON.stringify(
      this.def.bootstrap.map((index) => {
        const node = new ScenarioNode(
          identity(this.#table, index, this.#realm),
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

  /**
   * The schemes node `index` can be set up with now: all it runs, but a
   * hub's while that hub is not founded, since its config is the founded
   * hub's rules.
   */
  #schemesNow(index: number): readonly string[] {
    const hubs = hubSchemes(this.def);
    return this.#schemes(index).filter((scheme) =>
      !hubs.includes(scheme) || this.#configs[scheme] !== undefined
    );
  }

  /** Where node `index`'s validator file is. */
  #validatorFile(index: number): string {
    const name = identity(this.#table, index).name;
    return join(dataDir(this.options.layout, name), VALIDATOR_FILE);
  }

  /**
   * Writes the validator file of node `index`, on every start, as its
   * owner would: it validates the hubs the scenario's sponsors found, and
   * follows their escrows on the fake ledger, whose URL is the same every
   * run.
   */
  async #writeValidatorFile(index: number): Promise<void> {
    const ids = sponsors(this.def).map((sponsor) =>
      identity(this.#table, sponsor).id
    );
    if (ids.length === 0 || !this.ledger) {
      throw new Error("a validator needs the scenario's hub and fake ledger");
    }
    const file = this.#validatorFile(index);
    const name = identity(this.#table, index).name;
    await mkdir(dataDir(this.options.layout, name), { recursive: true });
    const content = JSON.parse(
      validatorFile(ids[0], this.ledger.url, this.ledger.publishers),
    ) as Record<string, unknown>;
    await writeFile(file, JSON.stringify({ ...content, sponsors: ids }));
  }

  /** The running node `index` of a hub. */
  #hubNode(index: number): ScenarioNode {
    const node = this.nodes.find((node) => node.index === index);
    if (!node) throw new Error(`node ${index} of a hub is not running`);
    return node;
  }

  /**
   * Founds the scenario's hubs once its nodes run, as each sponsor's owner
   * would (spec/areas/hubs.md#founding-a-hub): each with its validators and
   * the terms of its asset, once the sponsor's owner book of XRP, which
   * funds every escrow, shows XRP; the hubs of one sponsor at once, as
   * nothing makes one wait for another. Then each member joins its hubs
   * with their rules. The dev environment starts a hub scenario from
   * scratch on every run (`startsFresh`), since its escrows stay on the
   * earlier run's fake ledger; should a sponsor keep foundings all the
   * same, they are taken as they are, in the order they were made, for its
   * hubs in the order the scenario names them, and members that joined
   * keep their schemes, so nothing is founded twice.
   */
  async #foundHubs(): Promise<void> {
    const hubs = hubsOf(this.def);
    if (hubs.length === 0) return;
    const founded = await Promise.all(
      sponsors(this.def).map((sponsor) =>
        this.#found(
          sponsor,
          hubs.filter((hub) => hub.sponsor === sponsor),
        )
      ),
    );
    for (const [hub, rules] of founded.flat()) {
      this.#configs[hubScheme(hub)] = hubConfig(rules);
    }
    for (const node of this.nodes) {
      if (!hubs.some((hub) => joins(this.def, hub, node.index))) continue;
      await node.addSchemes(this.#schemes(node.index), this.#configs);
    }
  }

  /**
   * Has node `index` found `hubs`, those it sponsors, or takes the
   * foundings it keeps for them; each hub with its rules once founded.
   */
  async #found(
    index: number,
    hubs: readonly ScenarioHub[],
  ): Promise<[ScenarioHub, string][]> {
    const sponsor = this.#hubNode(index);
    const kept = (await sponsor.rpc.call("listHubFoundings", {})).foundings
      .toSorted((a, b) => Number(a.startedUnixMs - b.startedUnixMs))
      .map((founding) => founding.nonce);
    if (kept.length < hubs.length) {
      await until(`${sponsor.name}'s XRP`, async () => {
        const { ledgers } = await sponsor.rpc.call("listLedgers", {});
        const book = ledgers.find((ledger) =>
          ledger.settlementScheme === HUB_OWNER_BOOK
        );
        return book?.connection === LedgerConnection.SYNCED &&
          book.ownerBalance > 0n;
      }, SPONSOR_FUNDED_WITHIN_MS);
    }
    const nonces: string[] = [];
    for (const [i, hub] of hubs.entries()) {
      if (i < kept.length) {
        nonces.push(kept[i]);
        continue;
      }
      const validators = hub.validators
        .map((validator) => this.#hubNode(validator))
        .map((node) => ({
          node: node.id,
          endpoint: `${node.hostname}:${node.httpsPort}`,
        }));
      const { founding } = await sponsor.rpc.call("foundHub", {
        validators,
        terms: hubTerms(hub),
        xrplServer: "",
      });
      if (!founding) throw new Error(`${sponsor.name} started no founding`);
      nonces.push(founding.nonce);
    }
    return await Promise.all(
      hubs.map(async (hub, i): Promise<[ScenarioHub, string]> => [
        hub,
        await this.#founded(sponsor, nonces[i], hubScheme(hub)),
      ]),
    );
  }

  /** The rules of `sponsor`'s founding `nonce`, once it is founded. */
  async #founded(
    sponsor: ScenarioNode,
    nonce: string,
    scheme: string,
  ): Promise<string> {
    let rules = "";
    // Where the founding stands, and what holds it up, as the sponsor last
    // said: the error if it is never founded.
    let where = "no founding";
    try {
      await until(`the hub of ${scheme} founded`, async () => {
        const { foundings } = await sponsor.rpc.call("listHubFoundings", {});
        const founding = foundings.find((founding) => founding.nonce === nonce);
        if (founding !== undefined) {
          const problem = founding.problem === ""
            ? ""
            : `: ${founding.problem}`;
          where = `stage ${founding.stage}${problem}`;
          if (founding.stage === HubFoundingStage.FOUNDED) {
            rules = founding.rules;
          }
        }
        return rules !== "";
      }, FOUNDED_WITHIN_MS);
    } catch (error) {
      throw new Error(
        `${sponsor.name} founded no hub of ${scheme} (${where})`,
        {
          cause: error,
        },
      );
    }
    return rules;
  }

  /** Whether node `index` is minted the scenario's token. */
  #minted(index: number): boolean {
    return minted(this.def, this.#schemes(index));
  }

  /**
   * The environment of node `index`: the bootstrap nodes, for a periphery
   * node the IDs of the nodes it is pinned to, and in a live scenario the
   * dev switch that lets the node run in the live realm, which only a build
   * with debug assertions honours (spec/decisions/mainnet-schemes.md).
   */
  #nodeEnv(index: number): Record<string, string> {
    const env: Record<string, string> = {
      DASSIE_BOOTSTRAP_NODES: this.#bootstrapNodes(),
    };
    if (live(this.def)) env.DASSIE_DEV_LIVE_REALM = "1";
    const periphery = peripheryOf(this.def, index);
    if (periphery !== undefined) {
      env.DASSIE_UPLINKS = periphery.uplinks
        .map((uplink) => identity(this.#table, uplink, this.#realm).id)
        .join(",");
    }
    if (validatesAny(this.def, index)) {
      env.DASSIE_VALIDATOR_CONFIG = this.#validatorFile(index);
    }
    return env;
  }

  async #startNode(index: number): Promise<ScenarioNode> {
    if (index >= MAX_NODES) throw new Error(`at most ${MAX_NODES} nodes`);
    const node = new ScenarioNode(
      identity(this.#table, index, this.#realm),
      this.#environment,
      this.#nodeEnv(index),
      peripheryOf(this.def, index) !== undefined,
    );
    this.nodes.push(node);
    const report = this.options.onNode ?? (() => {});
    try {
      report(node, "starting");
      if (validatesAny(this.def, index)) {
        await this.#writeValidatorFile(index);
      }
      await node.start();
      report(node, "setting-up");
      await node.setUp(
        this.#schemesNow(index),
        this.#configs,
        this.rates?.url,
        this.#assets,
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
    const node = await this.#startNode(this.nodes.length);
    await this.#fund(node);
    if (this.def.mint !== undefined && this.#minted(node.index)) {
      await this.mintToken(node, this.def.mint.token, this.def.mint.value);
    }
    return node;
  }

  /**
   * In a live scenario, deposits {@linkcode MAINNET_DEPOSIT_DROPS} into
   * `node`'s mainnet account once it shows its address, as its owner would:
   * mainnet has no faucet. Nothing for a node of a test scenario, or one
   * without a scheme on the XRP Ledger.
   */
  async #fund(node: ScenarioNode): Promise<void> {
    if (!live(this.def) || !this.ledger) return;
    const schemes = this.#schemes(node.index);
    if (!schemes.some((scheme) => XRPL_SCHEMES.includes(scheme))) return;
    let address = "";
    await until(`${node.name}'s mainnet address`, async () => {
      const { externalAssets } = await node.rpc.call("listLedgers", {});
      const account = externalAssets.find((asset) =>
        asset.id.startsWith("xrpl-account/")
      );
      address = account?.address ?? "";
      return address !== "";
    }, ADDRESS_WITHIN_MS);
    await this.deposit(address, MAINNET_DEPOSIT_DROPS);
  }

  /** Sends `drops` of XRP to `address` on the fake ledger, from outside. */
  async deposit(address: string, drops: bigint): Promise<void> {
    if (!this.ledger) throw new Error("the scenario runs no fake ledger");
    await depositXrp(this.ledger, this.caFile, address, drops);
  }

  /** Asks the fake ledger's issuer of its network's dollar, or of the
   * request's token, to act. */
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
