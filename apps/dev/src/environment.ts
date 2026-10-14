/**
 * The dev environment's state (spec 6 §6.7): the scenario running, its
 * nodes and those added, the build, and what is gathered from the nodes.
 * Commands run one at a time, in order; the dashboards hear of every
 * change.
 *
 * @module
 */

import { create } from "@bufbuild/protobuf";
import {
  type Build,
  BuildSchema,
  BuildState,
  type ClientMessage,
  type Hop,
  type Node as NodeMessage,
  NodeSchema,
  NodeState,
  type Peerings,
  PeeringsSchema,
  PeerListSchema,
  type SourcedLogLine,
  SourcedLogLineSchema,
  type State,
  StateSchema,
} from "@dassie/rpc/dev";
import { LogLevel } from "@dassie/rpc/proto";
import { Collector, LogRing, shortName } from "./collect.ts";
import { ControlHub } from "./control.ts";
import { dataDir, type Layout } from "./paths.ts";
import type { Stream } from "./process.ts";
import {
  DEFAULT_SCENARIO,
  findScenario,
  type NodePhase,
  Scenario,
  type ScenarioNode,
  SCENARIOS,
} from "./scenario/mod.ts";
import { MAX_NODES } from "./scenario/scenarios.ts";
import { recordPids } from "./stale.ts";
import { type Identity, identity, loadTable } from "./vanity.ts";

/** How often gathered logs and traffic go out to the dashboards. */
const FLUSH_MS = 100;
/** Time between added nodes, as the original. */
const ADDED_NODE_GAP_MS = 500;
/** The end of a failed node's output that the dashboard shows. */
const MAX_ERROR_BYTES = 4 * 1024;
/** The longest message of the dev server's own log. */
const MAX_HOST_MESSAGE_BYTES = 1024;
/** Commands waiting behind the one running; more are refused. */
const MAX_PENDING_COMMANDS = 4;
/** Hops waiting for the next flush; more are dropped. */
const MAX_PENDING_HOPS = 1_000;

export interface EnvironmentOptions {
  readonly layout: Layout;
  /** The node binary, as built now. */
  readonly dassie: () => string;
  readonly portOffset: number;
  /** The wallet's development build, which the nodes serve. */
  readonly walletDir: string;
  /** The proxy's port, if it runs. */
  readonly proxyPort?: number;
  /** `DEBUG` for the nodes. */
  readonly debug: string;
  /** Prints a line to the terminal. */
  readonly print: (line: string) => void;
}

interface NodeStatus {
  state: NodeState;
  lastError: string;
}

const PHASES: Record<NodePhase, NodeState> = {
  "starting": NodeState.STARTING,
  "setting-up": NodeState.SETTING_UP,
  "running": NodeState.RUNNING,
  "failed": NodeState.FAILED,
};

export class Environment {
  readonly hub: ControlHub;
  readonly #table = loadTable();
  readonly #logs = new LogRing();
  readonly #peerings = new Map<string, string[]>();
  readonly #collectors = new Map<string, Collector>();
  readonly #statuses = new Map<string, NodeStatus>();
  /** Nodes whose process runs, from the moment it is spawned. */
  readonly #live = new Map<string, ScenarioNode>();
  /** Identities derived so far: deriving one takes a while. */
  readonly #identities = new Map<number, Identity>();
  #scenarioId = DEFAULT_SCENARIO;
  #scenario?: Scenario;
  #added = 0;
  #build: Build = create(BuildSchema, { state: BuildState.IDLE });
  #queue: Promise<void> = Promise.resolve();
  #pending = 0;
  #pendingLogs: SourcedLogLine[] = [];
  #pendingHops: Hop[] = [];
  #flusher: ReturnType<typeof setInterval>;
  #stopping = false;
  /** Children other than nodes, for the PID file. */
  extraPids: () => number[] = () => [];

  constructor(readonly options: EnvironmentOptions) {
    this.hub = new ControlHub(
      {
        state: () => this.state(),
        logs: () => this.#logs.lines,
        peerings: () => this.#peeringsMessage(),
      },
      (message) => this.command(message),
      (message) => this.log(LogLevel.WARN, message),
    );
    this.#flusher = setInterval(() => this.#flush(), FLUSH_MS);
  }

  /**
   * The scenario last chosen, from the state directory, or `override`.
   * Whether that is another scenario than the data was left by, which then
   * must not be resumed.
   */
  async restoreScenario(override?: string): Promise<boolean> {
    const saved = await Deno.readTextFile(this.options.layout.scenarioFile)
      .then((text) => text.trim(), () => undefined);
    const def = findScenario(override ?? saved ?? DEFAULT_SCENARIO);
    this.#scenarioId = def && !def.unavailable ? def.id : DEFAULT_SCENARIO;
    if (saved === this.#scenarioId) return false;
    await Deno.writeTextFile(
      this.options.layout.scenarioFile,
      `${this.#scenarioId}\n`,
    );
    return saved !== undefined;
  }

  #identity(index: number): Identity {
    let found = this.#identities.get(index);
    if (!found) {
      found = identity(this.#table, index);
      this.#identities.set(index, found);
    }
    return found;
  }

  // ---- The dashboards' view ----

  state(): State {
    const nodes: NodeMessage[] = [];
    for (const [name, status] of this.#statuses) {
      const index = Number(name.slice(1)) - 1;
      const node = this.#identity(index);
      nodes.push(create(NodeSchema, {
        name,
        id: node.id,
        url: this.walletUrl(name),
        state: status.state,
        loginProofHex: node.loginProofHex,
        lastError: status.lastError,
      }));
    }
    return create(StateSchema, {
      scenario: this.#scenarioId,
      scenarios: SCENARIOS.map((def) => ({
        id: def.id,
        name: def.name,
        description: def.description,
        available: !def.unavailable,
        unavailable: def.unavailable ?? "",
      })),
      nodes,
      build: this.#build,
      proxy: this.options.proxyPort !== undefined,
    });
  }

  /** Where a node's wallet is: through the proxy, if it runs. */
  walletUrl(name: string): string {
    const index = Number(name.slice(1)) - 1;
    const proxy = this.options.proxyPort;
    if (proxy === undefined) {
      return `https://${name}.localhost:${
        5001 + index + this.options.portOffset
      }`;
    }
    return `https://${name}.localhost${proxy === 443 ? "" : `:${proxy}`}`;
  }

  #peeringsMessage(): Peerings {
    const peers: Peerings["peers"] = {};
    for (const [name, list] of this.#peerings) {
      peers[name] = create(PeerListSchema, { peers: list });
    }
    return create(PeeringsSchema, { peers });
  }

  #changed(): void {
    this.hub.broadcast({ case: "state", value: this.state() });
  }

  /** The IPC socket of node `name`, while it runs. */
  nodeSocket(name: string): string | undefined {
    const node = this.#scenario?.nodes.find((node) => node.name === name);
    return node?.daemon && this.#statuses.get(name)?.state ===
        NodeState.RUNNING
      ? node.socket
      : undefined;
  }

  /** The running nodes. */
  get nodes(): readonly ScenarioNode[] {
    return this.#scenario?.nodes ?? [];
  }

  // ---- Logs ----

  /** A line of the dev server's own log, also printed. */
  log(level: LogLevel, message: string): void {
    const text = message.length > MAX_HOST_MESSAGE_BYTES
      ? `${message.slice(0, MAX_HOST_MESSAGE_BYTES)}…`
      : message;
    this.options.print(`${LogLevel[level].toLowerCase()} ${text}`);
    this.#pendingLogs.push(create(SourcedLogLineSchema, {
      source: "host",
      line: {
        index: 0n,
        timeUnixMs: BigInt(Date.now()),
        level,
        target: "das:dev",
        message: text,
      },
    }));
  }

  /** A line a node printed: its warnings and errors, and any panic. */
  nodeLine(node: string, _stream: Stream, line: string): void {
    this.options.print(`[${node}] ${line}`);
  }

  #flush(): void {
    if (this.#pendingLogs.length > 0) {
      const lines = this.#pendingLogs;
      this.#pendingLogs = [];
      this.#logs.add(lines);
      this.hub.broadcastLogs(lines);
    }
    if (this.#pendingHops.length > 0) {
      const hops = this.#pendingHops;
      this.#pendingHops = [];
      this.hub.broadcast({ case: "traffic", value: { hops } });
    }
  }

  // ---- Commands ----

  /** Runs `step` after every command before it, if not too many wait. */
  #enqueue(what: string, step: () => Promise<void>): Promise<void> {
    if (this.#pending >= MAX_PENDING_COMMANDS) {
      this.log(
        LogLevel.WARN,
        `${what} refused: ${this.#pending} commands wait`,
      );
      return this.#queue;
    }
    this.#pending++;
    const run = this.#queue.then(step).catch((error) => {
      this.log(LogLevel.ERROR, `${what} failed: ${error?.message ?? error}`);
    }).finally(() => this.#pending--);
    this.#queue = run;
    return run;
  }

  /** A dashboard's command. */
  command(message: ClientMessage): void {
    switch (message.message.case) {
      case "setScenario":
        void this.setScenario(message.message.value.id);
        break;
      case "addNode":
        void this.addNode();
        break;
      case "restartNodes":
        void this.restart();
        break;
      case "reset":
        void this.reset();
        break;
    }
  }

  setScenario(id: string): Promise<void> {
    const def = findScenario(id);
    if (!def || def.unavailable) {
      this.log(
        LogLevel.WARN,
        `cannot run scenario ${id}: ${def?.unavailable ?? "no such scenario"}`,
      );
      return Promise.resolve();
    }
    return this.#enqueue(`scenario ${id}`, async () => {
      this.#scenarioId = id;
      this.#added = 0;
      await Deno.writeTextFile(this.options.layout.scenarioFile, `${id}\n`);
      await this.#run(true);
    });
  }

  /** Starts the scenario from scratch. */
  reset(): Promise<void> {
    return this.#enqueue("reset", async () => {
      this.#added = 0;
      await this.#run(true);
    });
  }

  /** Restarts every node with its data, as after a new build. */
  restart(): Promise<void> {
    return this.#enqueue("restart", () => this.#run(false));
  }

  /**
   * Starts the scenario, keeping data from before but added nodes', or
   * from scratch if `fresh`.
   */
  start(fresh: boolean): Promise<void> {
    return this.#enqueue("start", async () => {
      this.#added = 0;
      await this.#wipeAdded();
      await this.#run(fresh);
    });
  }

  addNode(): Promise<void> {
    return this.#enqueue("add node", async () => {
      const scenario = this.#scenario;
      if (!scenario) throw new Error("no scenario is running");
      if (scenario.nodes.length >= MAX_NODES) {
        throw new Error(`at most ${MAX_NODES} nodes`);
      }
      await this.#addOne(scenario);
      this.#added++;
    });
  }

  async #addOne(scenario: Scenario): Promise<void> {
    const node = await scenario.addNode();
    this.#follow(node);
  }

  /**
   * Stops everything, once the command running is done; commands queued
   * behind it find no scenario and do nothing.
   */
  async stop(): Promise<void> {
    this.#stopping = true;
    await this.#queue;
    await this.#stopScenario();
    clearInterval(this.#flusher);
    this.#flush();
    this.hub.close();
  }

  // ---- Running ----

  async #run(wipe: boolean): Promise<void> {
    await this.#stopScenario();
    if (this.#stopping) return;
    if (wipe) await this.#wipe();
    const def = findScenario(this.#scenarioId)!;
    this.#statuses.clear();
    for (let index = 0; index < def.schemes.length + this.#added; index++) {
      this.#statuses.set(`d${index + 1}`, {
        state: NodeState.STOPPED,
        lastError: "",
      });
    }
    this.#changed();
    this.log(LogLevel.INFO, `starting ${def.id}${wipe ? " from scratch" : ""}`);
    let scenario: Scenario;
    try {
      scenario = await Scenario.start(def.id, {
        dassie: this.options.dassie(),
        layout: this.options.layout,
        portOffset: this.options.portOffset,
        env: {
          DASSIE_DEV_WALLET_DIR: this.options.walletDir,
          DASSIE_LOG_LEVEL: "warn",
          DEBUG: this.options.debug,
        },
        onLine: (node, stream, line) => this.nodeLine(node, stream, line),
        onSpawn: (node) => this.#spawned(node),
        onNode: (node, phase, error) => this.#phase(node, phase, error),
      });
    } catch (error) {
      this.log(
        LogLevel.ERROR,
        `${def.id} did not start: ${(error as Error).message}`,
      );
      this.log(
        LogLevel.INFO,
        "fix the cause and restart, or reset the scenario",
      );
      // The scenario stopped every node it had started; only the one that
      // failed says why.
      for (const status of this.#statuses.values()) {
        if (status.state !== NodeState.FAILED) status.state = NodeState.STOPPED;
        else if (status.lastError === "") {
          status.lastError = String((error as Error).message)
            .slice(-MAX_ERROR_BYTES);
        }
      }
      this.#changed();
      await this.recordPids();
      return;
    }
    this.#scenario = scenario;
    for (const node of scenario.nodes) this.#follow(node);
    for (let i = 0; i < this.#added && !this.#stopping; i++) {
      await new Promise((resolve) => setTimeout(resolve, ADDED_NODE_GAP_MS));
      await this.#addOne(scenario);
    }
    this.log(LogLevel.INFO, `${def.id} is running`);
  }

  /** A node's process runs: into the PID file at once. */
  #spawned(node: ScenarioNode): void {
    this.#live.set(node.name, node);
    void this.recordPids();
  }

  #phase(node: ScenarioNode, phase: NodePhase, error?: string): void {
    const status = this.#statuses.get(node.name) ??
      { state: NodeState.STOPPED, lastError: "" };
    status.state = PHASES[phase];
    if (phase === "failed") {
      status.lastError = (error ?? "").slice(-MAX_ERROR_BYTES);
      this.#live.delete(node.name);
      void this.recordPids();
    }
    this.#statuses.set(node.name, status);
    this.#changed();
  }

  /** Gathers from a running node, and notices if it dies. */
  #follow(node: ScenarioNode): void {
    const collector = new Collector(node.name, node.rpc, {
      logs: (lines) => this.#pendingLogs.push(...lines),
      peers: (name, peers) => {
        this.#peerings.set(name, peers);
        this.hub.broadcast({
          case: "peerings",
          value: this.#peeringsMessage(),
        });
      },
      traffic: (hops) => {
        const room = MAX_PENDING_HOPS - this.#pendingHops.length;
        if (room > 0) this.#pendingHops.push(...hops.slice(0, room));
      },
      failed: (name, what, message) =>
        this.log(LogLevel.WARN, `${name}: no ${what}: ${message}`),
    }, shortName);
    collector.start();
    this.#collectors.set(node.name, collector);
    const daemon = node.daemon;
    daemon?.exited.then((exit) => {
      this.#live.delete(node.name);
      void this.recordPids();
      // Stopped on purpose, or everything is stopping (ctrl+c reaches the
      // nodes too): not a crash.
      if (this.#stopping) return;
      if (this.#collectors.get(node.name) !== collector) return;
      collector.stop();
      this.#collectors.delete(node.name);
      this.#peerings.delete(node.name);
      this.#statuses.set(node.name, {
        state: NodeState.FAILED,
        lastError: exit.output.slice(-MAX_ERROR_BYTES),
      });
      this.log(LogLevel.ERROR, `${node.name} exited with ${exit.code}`);
      this.#changed();
    });
  }

  async #stopScenario(): Promise<void> {
    for (const collector of this.#collectors.values()) collector.stop();
    this.#collectors.clear();
    const scenario = this.#scenario;
    this.#scenario = undefined;
    if (scenario) {
      const failures = await scenario.stop();
      for (const failure of failures) this.log(LogLevel.WARN, failure);
    }
    this.#peerings.clear();
    for (const status of this.#statuses.values()) {
      if (status.state !== NodeState.FAILED) status.state = NodeState.STOPPED;
    }
    this.#live.clear();
    this.hub.broadcast({ case: "peerings", value: this.#peeringsMessage() });
    this.#changed();
    await this.recordPids();
  }

  async #wipe(): Promise<void> {
    await Deno.remove(this.options.layout.data, { recursive: true })
      .catch(() => {});
  }

  /** Added nodes do not outlive the dev server. */
  async #wipeAdded(): Promise<void> {
    const def = findScenario(this.#scenarioId)!;
    for (let index = def.schemes.length; index < MAX_NODES; index++) {
      const dir = dataDir(this.options.layout, `d${index + 1}`);
      await Deno.remove(dir, { recursive: true }).catch(() => {});
    }
  }

  /** Writes the PIDs of every child running, for the next start. */
  async recordPids(): Promise<void> {
    const pids = [
      ...this.extraPids(),
      ...[...this.#live.values()].flatMap((node) =>
        node.daemon ? [node.daemon.pid] : []
      ),
    ];
    await recordPids(this.options.layout.pidsFile, pids).catch(() => {});
  }

  // ---- The build ----

  buildStarted(): void {
    this.#build = create(BuildSchema, {
      ...this.#build,
      state: BuildState.BUILDING,
    });
    this.#changed();
  }

  buildFinished(result: { ok: true } | { ok: false; excerpt: string }): void {
    this.#build = create(BuildSchema, {
      state: result.ok ? BuildState.IDLE : BuildState.FAILED,
      errorExcerpt: result.ok ? "" : result.excerpt,
      finishedAtUnixMs: BigInt(Date.now()),
    });
    this.#changed();
  }
}
