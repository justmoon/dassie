/**
 * The fake XRP Ledger (M7b): the node's own model of the ledger, served
 * over WebSocket with a faucet over HTTPS, for scenarios that settle on
 * the XRP Ledger without the public testnet. CI's end-to-end tests run
 * against it, and `deno task dev --xrpl fake`.
 *
 * @module
 */

import { TextLineStream } from "@std/streams";
import { ROOT } from "./paths.ts";

/** The binary `cargo build --example fake_ledger` makes, by profile. */
export function fakeLedgerBinary(profile: "debug" | "e2e" = "debug"): string {
  return `${ROOT}target/${profile}/examples/fake_ledger`;
}

/** How long the fake ledger may take to start. */
const START_TIMEOUT_MS = 30_000;

/**
 * Where the fake ledger listens, with `portOffset` added: the same port
 * every run, since the nodes keep it in their config. The faucet is on the
 * next.
 */
export const FAKE_LEDGER_PORT = 10200;

/** A running fake ledger. */
export interface FakeLedger {
  /** Its WebSocket URL. */
  readonly url: string;
  /** Its faucet's URL. */
  readonly faucet: string | null;
  /** The process, for the PID file. */
  readonly pid: number;
  /** Stops it. */
  stop(): Promise<void>;
}

/** What an `xrpl-testnet` scheme is set up with to use `ledger`. */
export function xrplConfig(ledger: Pick<FakeLedger, "url" | "faucet">): string {
  return JSON.stringify({ server: ledger.url, faucet: ledger.faucet });
}

/** Builds the fake ledger, in the dev profile. */
export async function buildFakeLedger(): Promise<void> {
  const { success, stderr } = await new Deno.Command("cargo", {
    args: [
      "build",
      "--locked",
      "-p",
      "dassie-node",
      "--features",
      "testing",
      "--example",
      "fake_ledger",
    ],
    cwd: ROOT,
    stdout: "null",
    stderr: "piped",
  }).output();
  if (!success) {
    throw new Error(
      `the fake ledger did not build:\n${new TextDecoder().decode(stderr)}`,
    );
  }
}

/**
 * Starts `binary` on {@linkcode FAKE_LEDGER_PORT} plus `portOffset`, with
 * its faucet on HTTPS under the certificate in `certificateFile` and
 * `keyFile`, closing a ledger every `closeMs`. It is a new ledger every
 * time: nodes that knew an earlier one find their accounts gone, as after
 * a testnet reset, and the faucet funds them again.
 */
export async function startFakeLedger(
  binary: string,
  certificate: { certificateFile: string; keyFile: string },
  portOffset = 0,
  closeMs = 1_000,
): Promise<FakeLedger> {
  const port = FAKE_LEDGER_PORT + portOffset;
  const child = new Deno.Command(binary, {
    args: [
      "--listen",
      `127.0.0.1:${port}`,
      "--faucet-listen",
      `127.0.0.1:${port + 1}`,
      "--cert",
      certificate.certificateFile,
      "--key",
      certificate.keyFile,
      "--close-ms",
      `${closeMs}`,
    ],
    stdin: "piped",
    stdout: "piped",
    stderr: "inherit",
  }).spawn();
  const lines = child.stdout
    .pipeThrough(new TextDecoderStream())
    .pipeThrough(new TextLineStream())
    .getReader();
  const timer = setTimeout(() => child.kill(), START_TIMEOUT_MS);
  const first = await lines.read().finally(() => clearTimeout(timer));
  if (first.done) {
    await child.status;
    throw new Error("the fake ledger did not start");
  }
  const { url, faucet } = JSON.parse(first.value) as {
    url: string;
    faucet: string | null;
  };
  // Its stdout is not read again; let it drain.
  void (async () => {
    while (!(await lines.read()).done);
  })().catch(() => {});
  return {
    url,
    faucet,
    pid: child.pid,
    async stop() {
      // Closing stdin stops it; a kill if it does not.
      await child.stdin.close().catch(() => {});
      const exited = child.status.then(() => true);
      const late = new Promise<boolean>((resolve) =>
        setTimeout(() => resolve(false), 5_000)
      );
      if (!(await Promise.race([exited, late]))) {
        child.kill();
        await child.status;
      }
    },
  };
}
