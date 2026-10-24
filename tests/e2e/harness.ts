/**
 * What the end-to-end tests run: the release-like `dassie` binary, Pebble
 * (Let's Encrypt's test ACME server) with its DNS stand-in, and Chrome.
 *
 * The tools come from the Nix shell. Without them the tests are skipped,
 * except in CI, where a missing tool is a failure.
 */

import { Buffer } from "node:buffer";
import { type ChildProcess, spawn } from "node:child_process";
import { stat } from "node:fs/promises";
import { constants } from "node:os";
import process from "node:process";

import puppeteer, { type Browser } from "puppeteer-core";
import { waitForPort } from "@dassie/dev/scenario";

export { waitForPort, waitForSocket } from "@dassie/dev/scenario";

const root = new URL("../../", import.meta.url).pathname;

/** The binary under test, built with the wallet by `node --run build:e2e`. */
export const DASSIE = `${root}target/e2e/dassie`;
/** The fake XRP Ledger, built with it by `node --run build:e2e`. */
export const FAKE_LEDGER = `${root}target/e2e/examples/fake_ledger`;
/** Pebble's configuration and its API certificate. */
export const PEBBLE_DIR = `${root}testdata/pebble`;
/** The root of Pebble's API certificate, which the daemon must trust. */
export const PEBBLE_API_ROOT = `${PEBBLE_DIR}/root.pem`;
/** Pebble's ACME directory. */
export const PEBBLE_DIRECTORY = "https://localhost:14000/dir";
/**
 * `DASSIE_BOOTSTRAP_NODES` for a daemon under test: none, so that it stays
 * off the public testnet the default list names.
 */
export const NO_BOOTSTRAP_NODES = "[]";
/**
 * `DASSIE_RELAY` for a daemon under test: none, so that a node without a
 * hostname never dials the public front door, the default relay
 * (spec/decisions/default-relay.md).
 */
export const NO_RELAY = "";

/** The ports Pebble validates http-01 on (pebble-config.json) and HTTPS. */
export const HTTP_PORT = 18780;
export const HTTPS_PORT = 18743;

/** Where a child runs, and what it gets beside this process's environment. */
export interface ProcessOptions {
  cwd?: string;
  env?: Record<string, string>;
}

/** How a child exited, and what it wrote. */
export interface Output {
  code: number;
  stdout: string;
  stderr: string;
}

function start(
  command: string,
  args: string[],
  options: ProcessOptions,
): ChildProcess {
  return spawn(command, args, {
    cwd: options.cwd,
    env: { ...process.env, ...options.env },
    stdio: ["ignore", "pipe", "pipe"],
  });
}

/**
 * Collects what `child` writes until it exits. Killed by a signal, its code
 * is 128 and the signal's number, as a shell has it.
 */
function collect(child: ChildProcess): Promise<Output> {
  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  child.stdout?.on("data", (chunk: Buffer) => stdout.push(chunk));
  child.stderr?.on("data", (chunk: Buffer) => stderr.push(chunk));
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code, signal) =>
      resolve({
        code: code ?? 128 + constants.signals[signal!],
        stdout: Buffer.concat(stdout).toString(),
        stderr: Buffer.concat(stderr).toString(),
      }));
  });
}

/** Runs `command` to its end. */
export function run(
  command: string,
  args: string[],
  options: ProcessOptions = {},
): Promise<Output> {
  return collect(start(command, args, options));
}

async function which(name: string): Promise<string | undefined> {
  const result = await run("which", [name]);
  return result.code === 0 ? result.stdout.trim() : undefined;
}

export async function findChrome(): Promise<string | undefined> {
  const configured = process.env.CHROME_BIN;
  if (configured) return configured;
  for (const name of ["google-chrome", "chromium", "chromium-browser"]) {
    const path = await which(name);
    if (path) return path;
  }
  return undefined;
}

/** Why the tests cannot run here, if they cannot. */
export async function missingTools(): Promise<string | undefined> {
  const missing = [];
  for (
    const tool of [
      "pebble",
      "pebble-challtestsrv",
      "systemd-socket-activate",
      "mkcert",
    ]
  ) {
    if (!(await which(tool))) missing.push(tool);
  }
  if (!(await findChrome())) missing.push("Chrome (CHROME_BIN)");
  try {
    await stat(DASSIE);
  } catch {
    missing.push(`${DASSIE} (node --run build:e2e)`);
  }
  if (missing.length === 0) return undefined;
  const reason = `missing ${missing.join(", ")}`;
  if (process.env.CI) {
    throw new Error(`end-to-end tests cannot run: ${reason}`);
  }
  return reason;
}

/**
 * The ports of the scenarios' nodes are moved by this much, away from the
 * dev environment's (d1 has 18001 and 19001). Test files run in parallel,
 * so each one that starts a scenario adds its own hundred: scenarios +0,
 * market maker +100, DSUSD +200, open payments +300, third party +400, cross currency +500,
 * xrpl +600, wallet +700, xrpl deposit +800, market maker with two targets
 * +1000.
 */
export const SCENARIO_PORT_OFFSET = 14_000;

/**
 * Why the scenario tests cannot run here, if they cannot: they need the
 * binary, and mkcert for the nodes' certificates.
 */
export async function missingScenarioTools(): Promise<string | undefined> {
  const missing = [];
  if (!(await which("mkcert"))) missing.push("mkcert");
  for (const binary of [DASSIE, FAKE_LEDGER]) {
    try {
      await stat(binary);
    } catch {
      missing.push(`${binary} (node --run build:e2e)`);
    }
  }
  if (missing.length === 0) return undefined;
  const reason = `missing ${missing.join(", ")}`;
  if (process.env.CI) {
    throw new Error(`end-to-end tests cannot run: ${reason}`);
  }
  return reason;
}

/** Why tests that need only the binary cannot run here, if they cannot. */
export async function missingBinary(): Promise<string | undefined> {
  try {
    await stat(DASSIE);
    return undefined;
  } catch {
    const reason = `missing ${DASSIE} (node --run build:e2e)`;
    if (process.env.CI) {
      throw new Error(`end-to-end tests cannot run: ${reason}`);
    }
    return reason;
  }
}

/** A child process that is stopped with SIGTERM and awaited. */
export class Process {
  readonly #child: ChildProcess;
  readonly #output: Promise<Output>;
  readonly name: string;

  constructor(
    name: string,
    command: string,
    args: string[],
    options: ProcessOptions = {},
  ) {
    this.name = name;
    this.#child = start(command, args, options);
    this.#output = collect(this.#child);
  }

  /** Stops the process and returns its exit code and output. */
  async stop(): Promise<{ code: number; output: string }> {
    // Nothing happens if it exited already.
    this.#child.kill("SIGTERM");
    const { code, stdout, stderr } = await this.#output;
    return { code, output: stdout + stderr };
  }
}

/** Starts Pebble and its DNS stand-in, which resolves every name to 127.0.0.1. */
export async function startPebble(): Promise<Process[]> {
  const dns = new Process("challtestsrv", "pebble-challtestsrv", [
    "-defaultIPv4",
    "127.0.0.1",
    "-defaultIPv6",
    "",
    "-dns01",
    "127.0.0.1:18753",
    "-management",
    "127.0.0.1:18755",
    "-http01",
    "",
    "-https01",
    "",
    "-tlsalpn01",
    "",
    "-doh",
    "",
  ]);
  const pebble = new Process("pebble", "pebble", [
    "-config",
    "pebble-config.json",
    "-dnsserver",
    "127.0.0.1:18753",
  ], { cwd: PEBBLE_DIR, env: { PEBBLE_VA_NOSLEEP: "1" } });
  await waitForPort(14000);
  return [dns, pebble];
}

/**
 * Starts the daemon as systemd would: socket-activated, with the sockets
 * named as in the unit files.
 */
export async function startActivated(dir: string): Promise<Process> {
  const daemon = new Process("dassie", "systemd-socket-activate", [
    "--now",
    "-l",
    `127.0.0.1:${HTTP_PORT}`,
    "-l",
    `127.0.0.1:${HTTPS_PORT}`,
    "-l",
    `${dir}/ipc.sock`,
    "--fdname=dassie-http.socket:dassie-https.socket:dassie-ipc.socket",
    `--setenv=DASSIE_STATE_DIRECTORY=${dir}/state`,
    `--setenv=DASSIE_ACME_CA_FILE=${PEBBLE_API_ROOT}`,
    `--setenv=DASSIE_BOOTSTRAP_NODES=${NO_BOOTSTRAP_NODES}`,
    `--setenv=DASSIE_RELAY=${NO_RELAY}`,
    DASSIE,
    "daemon",
  ]);
  await waitForPort(HTTPS_PORT);
  return daemon;
}

/** Runs the CLI against the daemon in `dir`, and returns its stdout. */
export async function dassie(dir: string, args: string[]): Promise<string> {
  const result = await run(DASSIE, args, {
    env: { DASSIE_IPC_SOCKET_PATH: `${dir}/ipc.sock` },
  });
  if (result.code !== 0) {
    throw new Error(
      `dassie ${args.join(" ")} failed:\n${result.stdout}${result.stderr}`,
    );
  }
  return result.stdout;
}

/** A headless Chrome that sends `dassie.test` to this machine. */
export async function startBrowser(dir: string): Promise<Browser> {
  return await puppeteer.launch({
    executablePath: await findChrome(),
    headless: true,
    args: [
      "--no-sandbox",
      `--user-data-dir=${dir}/chrome`,
      "--host-resolver-rules=MAP dassie.test 127.0.0.1",
      // The chain is checked against Pebble's root separately, with
      // node:tls; Chrome would need the root in its NSS database for that.
      "--ignore-certificate-errors",
    ],
  });
}
