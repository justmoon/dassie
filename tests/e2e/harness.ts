/**
 * What the end-to-end tests run: the release-like `dassie` binary, Pebble
 * (Let's Encrypt's test ACME server) with its DNS stand-in, and Chrome.
 *
 * The tools come from the Nix shell. Without them the tests are skipped,
 * except in CI, where a missing tool is a failure.
 */

import { launch } from "@astral/astral";

const root = new URL("../../", import.meta.url).pathname;

/** The binary under test, built with the wallet by `deno task build:e2e`. */
export const DASSIE = `${root}target/e2e/dassie`;
/** Pebble's configuration and its API certificate. */
export const PEBBLE_DIR = `${root}testdata/pebble`;
/** The root of Pebble's API certificate, which the daemon must trust. */
export const PEBBLE_API_ROOT = `${PEBBLE_DIR}/root.pem`;
/** Pebble's ACME directory. */
export const PEBBLE_DIRECTORY = "https://localhost:14000/dir";

/** The ports Pebble validates http-01 on (pebble-config.json) and HTTPS. */
export const HTTP_PORT = 18780;
export const HTTPS_PORT = 18743;

async function which(name: string): Promise<string | undefined> {
  const result = await new Deno.Command("which", {
    args: [name],
    stdout: "piped",
    stderr: "null",
  })
    .output();
  return result.success
    ? new TextDecoder().decode(result.stdout).trim()
    : undefined;
}

export async function findChrome(): Promise<string | undefined> {
  const configured = Deno.env.get("CHROME_BIN");
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
    const tool of ["pebble", "pebble-challtestsrv", "systemd-socket-activate"]
  ) {
    if (!(await which(tool))) missing.push(tool);
  }
  if (!(await findChrome())) missing.push("Chrome (CHROME_BIN)");
  try {
    await Deno.stat(DASSIE);
  } catch {
    missing.push(`${DASSIE} (deno task build:e2e)`);
  }
  if (missing.length === 0) return undefined;
  const reason = `missing ${missing.join(", ")}`;
  if (Deno.env.get("CI")) {
    throw new Error(`end-to-end tests cannot run: ${reason}`);
  }
  return reason;
}

/** Why tests that need only the binary cannot run here, if they cannot. */
export async function missingBinary(): Promise<string | undefined> {
  try {
    await Deno.stat(DASSIE);
    return undefined;
  } catch {
    const reason = `missing ${DASSIE} (deno task build:e2e)`;
    if (Deno.env.get("CI")) {
      throw new Error(`end-to-end tests cannot run: ${reason}`);
    }
    return reason;
  }
}

/** A child process that is stopped with SIGTERM and awaited. */
export class Process {
  readonly #child: Deno.ChildProcess;
  readonly #output: Promise<Deno.CommandOutput>;
  readonly name: string;

  constructor(
    name: string,
    command: string,
    args: string[],
    options: Deno.CommandOptions = {},
  ) {
    this.name = name;
    this.#child = new Deno.Command(command, {
      args,
      stdin: "null",
      stdout: "piped",
      stderr: "piped",
      ...options,
    }).spawn();
    this.#output = this.#child.output();
  }

  /** Stops the process and returns its exit code and output. */
  async stop(): Promise<{ code: number; output: string }> {
    try {
      this.#child.kill("SIGTERM");
    } catch {
      // It exited already.
    }
    const { code, stdout, stderr } = await this.#output;
    const decoder = new TextDecoder();
    return { code, output: decoder.decode(stdout) + decoder.decode(stderr) };
  }
}

/** Waits until a TCP port accepts connections. */
export async function waitForPort(
  port: number,
  timeoutMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      (await Deno.connect({ hostname: "127.0.0.1", port })).close();
      return;
    } catch (error) {
      if (Date.now() > deadline) {
        throw new Error(`port ${port} did not open: ${error}`);
      }
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
}

/**
 * Waits until the daemon publishes its IPC socket. It binds its TCP ports
 * first and publishes the socket only once it answers, so an open port does
 * not mean the socket is there yet.
 */
export async function waitForSocket(
  path: string,
  timeoutMs = 15_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const found = await Deno.stat(path).then(
      (info) => info.isSocket,
      () => false,
    );
    if (found) return;
    if (Date.now() > deadline) throw new Error(`${path} did not appear`);
    await new Promise((resolve) => setTimeout(resolve, 50));
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
    DASSIE,
    "daemon",
  ]);
  await waitForPort(HTTPS_PORT);
  return daemon;
}

/** Runs the CLI against the daemon in `dir`, and returns its stdout. */
export async function dassie(dir: string, args: string[]): Promise<string> {
  const result = await new Deno.Command(DASSIE, {
    args,
    stdin: "null",
    stdout: "piped",
    stderr: "piped",
    env: { DASSIE_IPC_SOCKET_PATH: `${dir}/ipc.sock` },
  }).output();
  const decoder = new TextDecoder();
  const stdout = decoder.decode(result.stdout);
  if (!result.success) {
    throw new Error(
      `dassie ${args.join(" ")} failed:\n${stdout}${
        decoder.decode(result.stderr)
      }`,
    );
  }
  return stdout;
}

/** A headless Chrome that sends `dassie.test` to this machine. */
export async function startBrowser(
  dir: string,
): Promise<Awaited<ReturnType<typeof launch>>> {
  const path = await findChrome();
  return await launch({
    path,
    args: [
      "--no-sandbox",
      `--user-data-dir=${dir}/chrome`,
      "--host-resolver-rules=MAP dassie.test 127.0.0.1",
      // The chain is checked against Pebble's root separately, with Deno's
      // TLS; Chrome would need the root in its NSS database for that.
      "--ignore-certificate-errors",
    ],
  });
}
