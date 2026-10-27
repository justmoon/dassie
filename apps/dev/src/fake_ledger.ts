/**
 * The fake XRP Ledger: the node's own model of the ledger, served over
 * WebSocket with a faucet over HTTPS, for scenarios that settle on the XRP
 * Ledger without the public testnet. CI's end-to-end tests run against it,
 * and `node --run dev -- --xrpl fake`. It is the testnet, or mainnet, which
 * has no faucet: there, XRP arrives as a deposit from outside, as an owner
 * sends it. Next to its faucet, the issuer of its network's dollar, DSUSD
 * on the testnet and RLUSD on mainnet, mints, freezes and claws back on
 * request (spec/areas/xrpl.md#tokens-in-the-fake-ledger): the only source
 * of dollar tokens a scenario has. Its validators sign every ledger. The
 * binary also takes requests to rotate a validator's key, have one sign
 * nothing, publish a new list, or make every connection lie, as a
 * dishonest proof server would
 * (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation),
 * at `control`; nothing here sends them, and they are for use by hand.
 *
 * @module
 */

import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { Agent, fetch } from "undici";
import { ROOT } from "./paths.ts";
import { run } from "./process.ts";

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

/** The XRP Ledger networks the fake ledger can be. */
export type XrplNetwork = "testnet" | "mainnet";

/** A running fake ledger. */
export interface FakeLedger {
  /** The network it is. */
  readonly network: XrplNetwork;
  /** Its WebSocket URL. */
  readonly url: string;
  /** Its faucet's URL; mainnet has none. */
  readonly faucet: string | null;
  /** Where the issuer of its dollar takes requests, with the faucet. */
  readonly tokens: string | null;
  /** Where XRP is deposited from outside, with the faucet. */
  readonly deposit: string | null;
  /** Where its validators and its servers take requests, with the faucet:
   * `/validators/…` and `/lie` under it, for use by hand. */
  readonly control: string | null;
  /** What a scheme's config names to trust its validators: their
   * publisher, whose list it serves with the faucet
   * (spec/designs/xrpl-validation.md#10-configuration-and-state). */
  readonly publishers: Record<string, unknown> | null;
  /** The process, for the PID file. */
  readonly pid: number;
  /** Stops it. */
  stop(): Promise<void>;
}

/** What a node's XRPL account is set up with to use `ledger`: its server,
 * its faucet, `null` on mainnet, and the publisher of its validators
 * (spec/areas/settlement.md#external-assets). */
export function xrplConfig(
  ledger: Pick<FakeLedger, "url" | "faucet" | "publishers">,
): string {
  return JSON.stringify({
    server: ledger.url,
    faucet: ledger.faucet,
    ...ledger.publishers,
  });
}

/** Builds the fake ledger, in the dev profile. */
export async function buildFakeLedger(): Promise<void> {
  const { success, stderr } = await run("cargo", [
    "build",
    "--locked",
    "-p",
    "dassie-node",
    "--features",
    "testing",
    "--example",
    "fake_ledger",
  ], { cwd: ROOT });
  if (!success) {
    throw new Error(
      `the fake ledger did not build:\n${new TextDecoder().decode(stderr)}`,
    );
  }
}

/**
 * Starts `binary` as `network` on {@linkcode FAKE_LEDGER_PORT} plus
 * `portOffset`, with its faucet on HTTPS under the certificate in
 * `certificateFile` and `keyFile`, closing a ledger every `closeMs`. It is
 * a new ledger every time: on the testnet, nodes that knew an earlier one
 * find their accounts gone, as after a testnet reset, and the faucet funds
 * them again. Mainnet never resets, so a node that knew an earlier one
 * stops: a scenario on it starts its nodes from scratch.
 */
export async function startFakeLedger(
  binary: string,
  certificate: { certificateFile: string; keyFile: string },
  portOffset = 0,
  closeMs = 1_000,
  network: XrplNetwork = "testnet",
): Promise<FakeLedger> {
  const port = FAKE_LEDGER_PORT + portOffset;
  const child = spawn(binary, [
    "--network",
    network,
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
  ], { stdio: ["pipe", "pipe", "inherit"] });
  // A binary that cannot start has no PID, and errs only later.
  child.on("error", () => {});
  if (child.pid === undefined) throw new Error(`cannot run ${binary}`);
  const pid = child.pid;
  const status = new Promise<void>((resolve) => {
    child.once("close", () => resolve());
  });
  // It may be gone when its stdin is closed.
  child.stdin.on("error", () => {});
  // Its stdout is read to its end, though only its first line is used.
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const timer = setTimeout(() => child.kill(), START_TIMEOUT_MS);
  const first = await new Promise<string | undefined>((resolve) => {
    lines.once("line", resolve);
    lines.once("close", () => resolve(undefined));
  }).finally(() => clearTimeout(timer));
  if (first === undefined) {
    await status;
    throw new Error("the fake ledger did not start");
  }
  const { url, faucet, tokens, deposit, control, publishers } = JSON.parse(
    first,
  ) as {
    url: string;
    faucet: string | null;
    tokens: string | null;
    deposit: string | null;
    control: string | null;
    publishers: Record<string, unknown> | null;
  };
  return {
    network,
    url,
    faucet,
    tokens,
    deposit,
    control,
    publishers,
    pid,
    async stop() {
      // Closing stdin stops it; a kill if it does not.
      child.stdin.end();
      const exited = status.then(() => true);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const late = new Promise<boolean>((resolve) => {
        timer = setTimeout(() => resolve(false), 5_000);
      });
      const done = await Promise.race([exited, late]);
      clearTimeout(timer);
      if (!done) {
        child.kill();
        await status;
      }
    },
  };
}

/** What the fake ledger's issuer of its network's dollar, or of the token
 * `token` names, can be asked to do, each in the next ledger: mint to an
 * address, freeze or deep-freeze a line or lift that, freeze every line,
 * set a transfer fee in billionths, claw back, or make or set the token's
 * pool with XRP (spec/areas/xrpl.md#pools-in-the-fake-ledger), its
 * trading fee in hundred-thousandths. Values are decimal dollars, such as
 * `"12.5"`. DSUSD is on the testnet only, RLUSD on mainnet only. */
export type IssuerRequest =
  & { token?: "DSUSD" | "RLUSD" }
  & (
    | { action: "mint"; destination: string; value: string }
    | { action: "freeze" | "deep-freeze"; account: string; on: boolean }
    | { action: "global-freeze"; on: boolean }
    | { action: "transfer-rate"; rate: number }
    | { action: "clawback"; account: string; value: string }
    | { action: "amm"; drops: string; value: string; fee: number }
  );

/**
 * Asks the issuer of `ledger`'s dollar (or of `request.token`), whose
 * certificate comes from the CA in `caFile`. It is queued once this
 * returns, and applies in the next ledger; one the ledger refuses, such as
 * a mint to an account without a line, changes nothing.
 */
export async function askIssuer(
  ledger: Pick<FakeLedger, "tokens">,
  caFile: string,
  request: IssuerRequest,
): Promise<void> {
  if (!ledger.tokens) throw new Error("the fake ledger has no issuer");
  const { action, ...body } = request;
  await post(
    `${ledger.tokens}/${action}`,
    caFile,
    body,
    `the issuer ${action}`,
  );
}

/**
 * Sends `drops` of XRP to `destination` on `ledger` from outside, as an
 * owner funds an account: on mainnet, which has no faucet, the only way. It
 * is queued once this returns, and applies in the next ledger.
 */
export async function depositXrp(
  ledger: Pick<FakeLedger, "deposit">,
  caFile: string,
  destination: string,
  drops: bigint,
): Promise<void> {
  if (!ledger.deposit) throw new Error("the fake ledger takes no deposits");
  await post(
    ledger.deposit,
    caFile,
    { destination, drops: `${drops}` },
    "a deposit",
  );
}

/** Posts `body` as JSON to `url`, under the CA in `caFile`; `what` names it
 * when it is refused. */
async function post(
  url: string,
  caFile: string,
  body: unknown,
  what: string,
): Promise<void> {
  const agent = new Agent({
    connect: { ca: await readFile(caFile, "utf8") },
  });
  try {
    const response = await fetch(url, {
      method: "POST",
      dispatcher: agent,
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    const text = await response.text();
    if (response.status !== 200) {
      throw new Error(`refused: ${what} (${response.status}) ${text}`);
    }
  } finally {
    await agent.close();
  }
}
