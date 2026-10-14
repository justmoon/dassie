/**
 * The dev environment's scenarios (spec 6 §6.8), headless: real `dassie`
 * binaries on localhost, each with a fixed identity, set up over HTTPS and
 * IPC without a browser, peering over the v1 protocol on dev timers.
 *
 * M5's `apps/dev` builds on this; the scenarios here are `two-nodes` and
 * `six-nodes`, both on the stub scheme with `d0` and `d1` as bootstrap
 * nodes.
 */

import { assertEquals } from "@std/assert";
import { encodeBase64Url } from "@std/encoding/base64url";
import { encodeHex } from "@std/encoding/hex";
import { RpcClient } from "@dassie/rpc";
import { ipcConnector } from "@dassie/rpc/ipc";
import { loginSecret, nodeId, publicKey } from "@dassie/identity-wasm";

import {
  DASSIE,
  PEBBLE_DIR,
  Process,
  waitForPort,
  waitForSocket,
} from "./harness.ts";

/** Pebble's test root and its leaf for localhost, which every node serves. */
export const ROOT = await Deno.readTextFile(`${PEBBLE_DIR}/root.pem`);
const CERTIFICATE = await Deno.readTextFile(`${PEBBLE_DIR}/cert.pem`);
const PRIVATE_KEY = await Deno.readTextFile(`${PEBBLE_DIR}/key.pem`);

/** The first node's ports; each next node's are two higher. */
const BASE_PORT = 19100;

/** The scenarios: how many nodes, and which are bootstrap nodes. */
export const SCENARIOS = {
  "one-node": { nodes: 1, bootstrap: [] },
  "two-nodes": { nodes: 2, bootstrap: [0, 1] },
  "six-nodes": { nodes: 6, bootstrap: [0, 1] },
} as const;

/** How a scenario starts. */
export interface ScenarioOptions {
  /** More environment for every node. */
  env?: Record<string, string>;
  /** Nodes left without a ledger, by index. */
  withoutLedger?: number[];
}

export type ScenarioName = keyof typeof SCENARIOS;

/** A node of a running scenario. */
export class ScenarioNode {
  readonly key: Uint8Array;
  readonly id: string;
  readonly httpPort: number;
  readonly httpsPort: number;
  daemon?: Process;
  owner?: RpcClient;

  constructor(
    readonly index: number,
    readonly dir: string,
    readonly env: Record<string, string>,
  ) {
    // A fixed key per index: the same node IDs on every run.
    this.key = new Uint8Array(32).fill(index + 101);
    this.id = nodeId(publicKey(this.key));
    this.httpPort = BASE_PORT + 2 * index;
    this.httpsPort = this.httpPort + 1;
  }

  get name(): string {
    return `d${this.index}`;
  }

  get url(): string {
    return `https://localhost:${this.httpsPort}`;
  }

  /** Its entry in `DASSIE_BOOTSTRAP_NODES`. */
  bootstrapEntry() {
    return {
      id: this.id,
      url: this.url,
      publicKey: encodeBase64Url(publicKey(this.key)),
    };
  }

  async start() {
    this.daemon = new Process(this.name, DASSIE, [
      "daemon",
      "--http-port",
      `${this.httpPort}`,
      "--https-port",
      `${this.httpsPort}`,
    ], {
      env: {
        ...this.env,
        DASSIE_STATE_DIRECTORY: `${this.dir}/state`,
        DASSIE_IPC_SOCKET_PATH: `${this.dir}/ipc.sock`,
      },
    });
    await waitForPort(this.httpsPort);
    await waitForSocket(`${this.dir}/ipc.sock`);
    this.owner = new RpcClient({
      url: "ipc",
      connect: ipcConnector(`${this.dir}/ipc.sock`),
    });
  }

  async stop() {
    this.owner?.close();
    this.owner = undefined;
    const result = await this.daemon?.stop();
    this.daemon = undefined;
    return result;
  }

  /** The owner's RPC client. */
  get rpc(): RpcClient {
    if (!this.owner) throw new Error(`${this.name} is not running`);
    return this.owner;
  }

  /**
   * Claims the node: certificate, hostname, setup, and the stub scheme
   * unless `ledger` is false.
   */
  async setUp(ledger = true) {
    const owner = this.rpc;
    await owner.call("setCertificate", {
      certificateChainPem: CERTIFICATE,
      privateKeyPem: PRIVATE_KEY,
    });
    await owner.call("setHostname", { hostname: "localhost" });
    const { url } = await owner.call("getSetupUrl", {});
    const token = url.split("/setup/")[1];
    const client = Deno.createHttpClient({ caCerts: [ROOT] });
    const response = await fetch(`${this.url}/api/setup`, {
      method: "POST",
      client,
      headers: { "content-type": "application/json", origin: this.url },
      body: JSON.stringify({
        setupAuthorizationToken: token,
        rawDassieKeyHex: encodeHex(this.key),
        loginAuthorizationSignature: encodeHex(loginSecret(this.key)),
      }),
    });
    assertEquals(response.status, 200, await response.text());
    client.close();
    await this.waitForEngine();
    if (!ledger) return;
    const added = await owner.call("addSettlementScheme", {
      id: "stub",
      config: "",
    });
    assertEquals(added.ledger?.ownerBalance, 100_000_000_000n);
  }

  /** Calls until the engine answers: it starts once the node is set up. */
  async waitForEngine() {
    const deadline = Date.now() + 15_000;
    for (;;) {
      try {
        return await this.rpc.call("listLedgers", {});
      } catch (error) {
        if (Date.now() > deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    }
  }
}

/** A running scenario. */
export class Scenario {
  constructor(readonly nodes: ScenarioNode[], readonly dir: string) {}

  /** Starts a scenario's nodes and sets each up. */
  static async start(
    name: ScenarioName,
    options: ScenarioOptions = {},
  ): Promise<Scenario> {
    const scenario = SCENARIOS[name];
    const dir = await Deno.makeTempDir({ prefix: `dassie-${name}-` });
    const nodes: ScenarioNode[] = [];
    const env: Record<string, string> = {
      DASSIE_PEER_CA_FILE: `${PEBBLE_DIR}/root.pem`,
      DASSIE_PEER_ALLOW_PRIVATE: "1",
      DASSIE_DEV_TIMINGS: "1",
      ...options.env,
    };
    for (let index = 0; index < scenario.nodes; index++) {
      nodes.push(new ScenarioNode(index, `${dir}/d${index}`, env));
    }
    env.DASSIE_BOOTSTRAP_NODES = JSON.stringify(
      (scenario.bootstrap as readonly number[]).map((index) =>
        nodes[index].bootstrapEntry()
      ),
    );
    const running = new Scenario(nodes, dir);
    try {
      for (const node of nodes) {
        await Deno.mkdir(node.dir, { recursive: true });
        await node.start();
        await node.setUp(!options.withoutLedger?.includes(node.index));
      }
    } catch (error) {
      await running.stop();
      throw error;
    }
    return running;
  }

  /** Stops every node and deletes their files; returns the output of those that failed. */
  async stop(): Promise<string[]> {
    const failures: string[] = [];
    for (const node of this.nodes) {
      const result = await node.stop();
      if (result && result.code !== 0) {
        failures.push(`${node.name} exited ${result.code}:\n${result.output}`);
      }
    }
    await Deno.remove(this.dir, { recursive: true }).catch(() => {});
    return failures;
  }

  /** Waits until `done` holds, polling. */
  async until(what: string, done: () => Promise<boolean>, timeoutMs = 90_000) {
    const deadline = Date.now() + timeoutMs;
    while (!(await done())) {
      if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
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
