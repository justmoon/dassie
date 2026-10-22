/**
 * A node of a scenario: the real `dassie` binary, with a vanity identity,
 * named `dN.localhost` on ports `4000+N` (HTTP), `5000+N` (HTTPS, and QUIC
 * on UDP) and `6000+N` (its rate feed, on UDP), and
 * set up the way its owner would set it up: over IPC and the HTTPS API,
 * never by writing its database (spec/areas/dev.md#setting-nodes-up).
 *
 * Setting up can run again on a node that is set up already: it then
 * checks that the data is this node's, and adds only what is missing.
 *
 * A periphery node is set up without its hostname, so it has no URL
 * (spec/areas/network.md#periphery-nodes); it still serves its owner over
 * HTTPS at its ports, and the dev environment reaches it as any other.
 *
 * @module
 */

import { assertEquals } from "@std/assert";
import { mkdir, readFile, rm } from "node:fs/promises";
import { RpcClient } from "@dassie/rpc";
import { ipcConnector } from "@dassie/rpc/ipc";
import { NodeState } from "@dassie/rpc/proto";
import { Agent, fetch } from "undici";
import { nodeCertificate } from "../certs.ts";
import { dataDir, type Layout, socketPath } from "../paths.ts";
import { ChildProcess, type Exit, type Stream } from "../process.ts";
import type { Identity } from "../vanity.ts";
import { waitForPort, waitForSocket } from "../wait.ts";

/** The first node's ports, less one: d1 has 4001, 5001 and 6001. */
export const HTTP_PORT_BASE = 4000;
export const HTTPS_PORT_BASE = 5000;
export const FEED_PORT_BASE = 6000;

/** How long a node has to come up, and its engine to start. */
const START_TIMEOUT_MS = 30_000;
const ENGINE_TIMEOUT_MS = 15_000;

/** What the nodes of one environment share. */
export interface NodeEnvironment {
  /** The `dassie` binary. */
  readonly dassie: string;
  readonly layout: Layout;
  /** The CA the nodes' certificates come from, which they all trust. */
  readonly caFile: string;
  /** Added to every port. */
  readonly portOffset: number;
  /** More environment for every node. */
  readonly env: Readonly<Record<string, string>>;
  /** Each line a node prints. */
  readonly onLine?: (node: string, stream: Stream, line: string) => void;
  /** Each node's process, as soon as it is spawned. */
  readonly onSpawn?: (node: ScenarioNode) => void;
}

export class ScenarioNode {
  readonly name: string;
  readonly id: string;
  readonly key: Uint8Array;
  readonly hostname: string;
  readonly httpPort: number;
  readonly httpsPort: number;
  /** Its rate feed's UDP port, on loopback. */
  readonly feedPort: number;
  readonly dir: string;
  readonly socket: string;
  daemon?: ChildProcess;
  #owner?: RpcClient;
  readonly identity: Identity;
  readonly environment: NodeEnvironment;
  /** Environment for this node only, such as the bootstrap nodes. */
  readonly env: Readonly<Record<string, string>>;
  /** Whether it is set up without its hostname, as a periphery node. */
  readonly periphery: boolean;

  constructor(
    identity: Identity,
    environment: NodeEnvironment,
    env: Readonly<Record<string, string>> = {},
    periphery = false,
  ) {
    this.identity = identity;
    this.environment = environment;
    this.env = env;
    this.periphery = periphery;
    this.name = identity.name;
    this.id = identity.id;
    this.key = identity.key;
    this.hostname = `${this.name}.localhost`;
    this.httpPort = HTTP_PORT_BASE + identity.index + 1 +
      environment.portOffset;
    this.httpsPort = HTTPS_PORT_BASE + identity.index + 1 +
      environment.portOffset;
    this.feedPort = FEED_PORT_BASE + identity.index + 1 +
      environment.portOffset;
    this.dir = dataDir(environment.layout, this.name);
    this.socket = socketPath(environment.layout, this.name);
  }

  get index(): number {
    return this.identity.index;
  }

  /** Its URL, as browsers and peers reach it. */
  get url(): string {
    return `https://${this.hostname}:${this.httpsPort}`;
  }

  /**
   * Its URL at `localhost`, which its certificate also names: the dev
   * server and the tests reach it there, without resolving `dN.localhost`.
   */
  get localUrl(): string {
    return `https://localhost:${this.httpsPort}`;
  }

  /** Its entry in `DASSIE_BOOTSTRAP_NODES`. */
  bootstrapEntry(): { id: string; url: string; publicKey: string } {
    return {
      id: this.id,
      url: this.url,
      publicKey: this.identity.publicKey.toBase64({
        alphabet: "base64url",
        omitPadding: true,
      }),
    };
  }

  /** Starts the daemon, and waits until it answers over IPC. */
  async start(): Promise<void> {
    if (this.daemon) throw new Error(`${this.name} is running`);
    await mkdir(this.dir, { recursive: true });
    await mkdir(this.environment.layout.sockets, {
      recursive: true,
      mode: 0o700,
    });
    // A socket left by a node that was killed would look published.
    await rm(this.socket).catch(() => {});
    const onLine = this.environment.onLine;
    this.daemon = new ChildProcess(this.name, this.environment.dassie, {
      args: [
        "daemon",
        "--http-port",
        `${this.httpPort}`,
        "--https-port",
        `${this.httpsPort}`,
        // Its key is public (the vanity table): loopback only.
        "--listen",
        "127.0.0.1",
        // Every node's feed on a port of its own, not the default one.
        "--feed-listen",
        `127.0.0.1:${this.feedPort}`,
      ],
      env: {
        // No relay unless the scenario names one: the default is the
        // public front door (spec/decisions/default-relay.md).
        DASSIE_RELAY: "",
        ...this.environment.env,
        ...this.env,
        DASSIE_STATE_DIRECTORY: `${this.dir}/state`,
        DASSIE_IPC_SOCKET_PATH: this.socket,
        DASSIE_PEER_CA_FILE: this.environment.caFile,
        DASSIE_PEER_ALLOW_PRIVATE: "1",
        DASSIE_DEV_TIMINGS: "1",
      },
      onLine: onLine && ((stream, line) => onLine(this.name, stream, line)),
    });
    this.environment.onSpawn?.(this);
    const gone = this.daemon.exited;
    try {
      await waitForPort(this.httpsPort, START_TIMEOUT_MS, gone);
      await waitForSocket(this.socket, START_TIMEOUT_MS, gone);
    } catch (error) {
      const exit = await this.stop();
      throw new Error(`${this.name} did not start: ${error}\n${exit?.output}`);
    }
    this.#owner = new RpcClient({
      url: "ipc",
      connect: ipcConnector(this.socket),
    });
  }

  /** Stops the daemon; how it ended, if it ran. */
  async stop(): Promise<Exit | undefined> {
    this.#owner?.close();
    this.#owner = undefined;
    const exit = await this.daemon?.stop();
    this.daemon = undefined;
    return exit;
  }

  /** The owner's RPC client, over IPC. */
  get rpc(): RpcClient {
    if (!this.#owner) throw new Error(`${this.name} is not running`);
    return this.#owner;
  }

  /**
   * Sets the node up as its owner would, and adds the settlement schemes in
   * `schemes` it does not have yet, each with its config in `configs`, if
   * any; its exchange rates come from `rates`, if given. On a node set up
   * before, it checks the node is this one and leaves the rest as it is.
   */
  async setUp(
    schemes: readonly string[],
    configs: Readonly<Record<string, string>> = {},
    rates?: string,
  ): Promise<void> {
    const owner = this.rpc;
    const certificate = await nodeCertificate(
      this.environment.layout.tls,
      this.name,
      this.environment.layout.caRoot,
    );
    // Again on every start: the CA may have been made anew.
    await owner.call("setCertificate", {
      certificateChainPem: certificate.certificatePem,
      privateKeyPem: certificate.keyPem,
    });
    // A periphery node has no hostname, so no URL: it only dials out.
    if (!this.periphery) {
      await owner.call("setHostname", { hostname: this.hostname });
    }
    const basic = await owner.call("getBasicState", {});
    if (basic.state === NodeState.UNINITIALIZED) {
      await this.#claim();
      await this.waitForEngine();
      await owner.call("setAlias", { alias: this.name });
    } else {
      assertEquals(
        basic.nodeId,
        this.id,
        `${this.dir} holds another node's data`,
      );
      await this.waitForEngine();
    }
    if (rates) {
      // Again on every start: the port may have moved.
      await owner.call("setRateSources", {
        sources: [{ kind: "coinbase", url: rates }],
      });
    }
    const active = (await owner.call("getBasicState", {}))
      .activeSettlementSchemes;
    for (const scheme of schemes) {
      if (active.includes(scheme)) continue;
      await owner.call("addSettlementScheme", {
        id: scheme,
        config: configs[scheme] ?? "",
      });
    }
  }

  /** Completes setup over HTTPS with the node's key, as the wallet does. */
  async #claim(): Promise<void> {
    const { url } = await this.rpc.call("getSetupUrl", {});
    const token = url.split("/setup/")[1];
    const agent = new Agent({
      connect: { ca: await readFile(this.environment.caFile, "utf8") },
    });
    try {
      const response = await fetch(`${this.localUrl}/api/setup`, {
        method: "POST",
        dispatcher: agent,
        headers: {
          "content-type": "application/json",
          origin: this.localUrl,
        },
        body: JSON.stringify({
          setupAuthorizationToken: token,
          rawDassieKeyHex: this.key.toHex(),
          loginAuthorizationSignature: this.identity.loginProofHex,
        }),
      });
      const text = await response.text();
      if (response.status !== 200) {
        throw new Error(
          `${this.name}: setup failed (${response.status}) ${text}`,
        );
      }
    } finally {
      await agent.close();
    }
  }

  /** Calls until the engine answers: it starts once the node is set up. */
  async waitForEngine(): Promise<void> {
    const deadline = Date.now() + ENGINE_TIMEOUT_MS;
    for (;;) {
      try {
        await this.rpc.call("listLedgers", {});
        return;
      } catch (error) {
        if (Date.now() > deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
  }
}
