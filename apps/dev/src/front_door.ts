/**
 * A front door for a scenario (spec/areas/dev.md#scenarios,
 * spec/areas/http.md#front-doors): `dassie front-door` on loopback, for a
 * domain of the scenario's own under its CA, which the scenario's periphery
 * nodes keep as their relay and answer to their names at. It reads no
 * registrar's names: a node is reached by its ID alone.
 *
 * Its names are on the default port, as a front door's always are, and no
 * resolver knows the domain: a client maps every name under it to the front
 * door's address and port: the end-to-end test does it for its client and,
 * through a proxy, for Chrome.
 *
 * @module
 */

import { mkdir } from "node:fs/promises";
import { certificate } from "./certs.ts";
import { ChildProcess, type Exit, run } from "./process.ts";
import { waitForPort } from "./wait.ts";

/** How long the front door may take to listen. */
const START_TIMEOUT_MS = 15_000;

/** What a scenario's front door runs with. */
export interface FrontDoorOptions {
  /** Its domain: nodes are at `<node ID>.<domain>`. */
  readonly domain: string;
  /** Its port on loopback, TCP for HTTPS and UDP for QUIC. */
  readonly port: number;
}

/** A running front door. */
export interface FrontDoor {
  readonly domain: string;
  readonly port: number;
  /** The `DASSIE_RELAY` entry that names it. */
  readonly entry: string;
  /** The name of the node with `id` at it. */
  name(id: string): string;
  stop(): Promise<Exit>;
}

/**
 * Starts `dassie` as a front door for `options.domain` on loopback, with
 * its key and its certificate, from the CA at `caRoot`, in `dir`.
 */
export async function startFrontDoor(
  dassie: string,
  dir: string,
  caRoot: string | undefined,
  options: FrontDoorOptions,
  onLine?: (stream: "stdout" | "stderr", line: string) => void,
): Promise<FrontDoor> {
  await mkdir(dir, { recursive: true, mode: 0o700 });
  const tls = await certificate(dir, "front-door", [
    `*.${options.domain}`,
    options.domain,
  ], caRoot);
  const listen = `127.0.0.1:${options.port}`;
  const env = {
    DASSIE_FRONT_DOOR_DOMAIN: options.domain,
    DASSIE_FRONT_DOOR_HTTPS_LISTEN: listen,
    DASSIE_FRONT_DOOR_QUIC_LISTEN: listen,
    DASSIE_FRONT_DOOR_CERTIFICATE: tls.certificateFile,
    DASSIE_FRONT_DOOR_CERTIFICATE_KEY: tls.keyFile,
    DASSIE_FRONT_DOOR_KEY_FILE: `${dir}/front-door.key`,
    DASSIE_FRONT_DOOR_RELAY_URL: `https://localhost:${options.port}`,
    // No registrars: names are not read, node IDs are.
    DASSIE_BOOTSTRAP_NODES: "[]",
    DASSIE_PEER_ALLOW_PRIVATE: "1",
    DASSIE_DEV_TIMINGS: "1",
  };
  const made = await run(dassie, ["front-door", "--make-key"], { env });
  if (!made.success) {
    throw new Error(
      `the front door's key: ${new TextDecoder().decode(made.stderr)}`,
    );
  }
  const entry = new TextDecoder().decode(made.stdout).trim();
  const child = new ChildProcess("front-door", dassie, {
    args: ["front-door"],
    env,
    onLine,
  });
  try {
    await waitForPort(options.port, START_TIMEOUT_MS, child.exited);
  } catch (error) {
    const exit = await child.stop();
    throw new Error(`the front door did not start: ${error}\n${exit.output}`);
  }
  return {
    domain: options.domain,
    port: options.port,
    entry,
    name: (id) => `${id}.${options.domain}`,
    stop: () => child.stop(),
  };
}
