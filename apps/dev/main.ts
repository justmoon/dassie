/**
 * `deno task dev` (spec 6 §6, spec 7 §6): a network of dev nodes on this
 * machine, and the dashboard to watch it at https://localhost.
 *
 * 1. Checks what it needs, and stops what a crashed run left behind.
 * 2. Builds the node (cargo), the wallet and the dashboard (Vite, in
 *    development mode, watching).
 * 3. Serves the dashboard with its control channel and the relay to the
 *    nodes, and the SNI proxy on 443.
 * 4. Runs the scenario chosen last, and rebuilds and restarts the nodes
 *    when the Rust sources change.
 *
 * Flags, for tests and special cases:
 *
 * - `--scenario <id>`: run this scenario instead of the one chosen last;
 * - `--dassie <path>`: run this binary, and do not build or watch the node;
 * - `--no-watch`: build the wallet and the dashboard once, watch nothing;
 * - `--no-proxy`: no SNI proxy on 443;
 * - `--headless`: no keys, no hints for a person;
 * - `--port-offset <n>`: move every port by n;
 * - `--state-dir <dir>`: keep the state there instead of `local/dev`
 *   (also `DASSIE_DEV_STATE_DIR`);
 * - `--xrpl <testnet|fake>`: where the XRPL scenarios settle: the public
 *   testnet (the default), or the fake ledger the tests use.
 *
 * @module
 */

import { mkdir, rm } from "node:fs/promises";
import process from "node:process";
import { parseArgs } from "node:util";
import { LogLevel } from "@dassie/rpc/proto";
import {
  CargoBuilder,
  DEV_BINARY,
  ViteBuild,
  watchSources,
} from "./src/build.ts";
import { caFile, dashboardCertificate } from "./src/certs.ts";
import { buildFakeLedger, fakeLedgerBinary } from "./src/fake_ledger.ts";
import { Environment } from "./src/environment.ts";
import { listenKeys, openBrowser } from "./src/keys.ts";
import { layout as makeLayout, ROOT } from "./src/paths.ts";
import { missingPrerequisites } from "./src/prereqs.ts";
import { run } from "./src/process.ts";
import { type Proxy, proxyHint, startProxy } from "./src/proxy.ts";
import { MAX_NODES } from "./src/scenario/scenarios.ts";
import { type Server, startServer } from "./src/server.ts";
import { upstreamPort } from "./src/sni.ts";
import { releaseLock, stopStale, takeLock } from "./src/stale.ts";

const DASHBOARD_PORT = 10100;
const CONTROL_PORT = 10101;
const PROXY_PORT = 443;

const args = flags();
const xrpl = args.xrpl ?? "testnet";
if (xrpl !== "testnet" && xrpl !== "fake") {
  console.error("--xrpl must be testnet or fake");
  process.exit(2);
}
const offset = Number(args["port-offset"] ?? "0");
const watch = !args["no-watch"];
if (!Number.isSafeInteger(offset) || offset < 0 || offset > 40_000) {
  console.error("--port-offset must be a number from 0 to 40000");
  process.exit(2);
}
const print = (line: string) => console.log(line);
const layout = await makeLayout(
  args["state-dir"] ?? process.env.DASSIE_DEV_STATE_DIR,
);
await mkdir(layout.state, { recursive: true });

print("Dassie//dev  Starting the development environment…");
const missing = await missingPrerequisites(layout, MAX_NODES);
if (missing.length > 0) {
  for (const line of missing) console.error(`  ✗ ${line}`);
  process.exit(1);
}
const other = await takeLock(layout.lockFile);
if (other !== undefined) {
  console.error(
    `another dev environment (PID ${other}) runs on ${layout.state}: stop it first`,
  );
  process.exit(1);
}
for (const stale of await stopStale(layout.pidsFile)) {
  print(`stopped ${stale}, left by an earlier run`);
}

// What runs so far, to stop on the way out, whenever that is.
let proxy: Proxy | undefined;
let environment: Environment | undefined;
let builds: ViteBuild[] = [];
let server: Server | undefined;
let watcher: { close(): void } | undefined;
let keys: { close(): void } | undefined;
let quitting: Promise<void> | undefined;

/** Stops everything; a second call quits at once. */
function quit(code = 0): Promise<void> {
  if (quitting) {
    print("quitting at once");
    process.exit(130);
  }
  quitting = (async () => {
    print("stopping…");
    keys?.close();
    watcher?.close();
    await environment?.stop();
    await Promise.all(builds.map((build) => build.stop()));
    proxy?.close();
    await server?.close();
    await rm(layout.pidsFile).catch(() => {});
    await releaseLock(layout.lockFile);
    process.exit(code);
  })();
  return quitting;
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => void quit());
}

try {
  await start();
} catch (error) {
  console.error(
    `the dev environment did not start: ${(error as Error).message}`,
  );
  await quit(1);
}

async function start(): Promise<void> {
  await caFile(layout.tls, layout.caRoot);
  const certificate = await dashboardCertificate(layout.tls, layout.caRoot);

  // The proxy first: the wallet links depend on whether it runs.
  const proxyPort = PROXY_PORT + offset;
  const dashboardPort = DASHBOARD_PORT + offset;
  if (!args["no-proxy"]) {
    try {
      proxy = await startProxy({
        port: proxyPort,
        upstream: (name) =>
          upstreamPort(name, {
            dashboard: dashboardPort,
            node: (index) => 5001 + index + offset,
          }, MAX_NODES),
        log: print,
      });
    } catch (error) {
      print(proxyHint(error, proxyPort));
    }
  }

  const dassie = args.dassie ?? DEV_BINARY;
  const env = new Environment({
    layout,
    dassie: () => dassie,
    portOffset: offset,
    walletDir: `${ROOT}apps/wallet/dist-dev`,
    proxyPort: proxy ? proxyPort : undefined,
    debug: process.env.DEBUG ?? "das:*",
    fakeLedger: xrpl === "fake" ? fakeLedgerBinary() : undefined,
    print,
  });
  environment = env;
  env.extraPids = () => builds.flatMap((build) => build.pids());

  const builder = args.dassie === undefined ? await buildNode(env) : undefined;
  if (xrpl === "fake") {
    print("building the fake XRP Ledger…");
    await buildFakeLedger();
  }
  await buildPages(env);

  server = startServer({
    ports: [dashboardPort, CONTROL_PORT + offset],
    certificatePem: certificate.certificatePem,
    keyPem: certificate.keyPem,
    dashboardDir: `${ROOT}apps/dev-dashboard/dist-dev`,
    control: env.hub,
    nodeSocket: (name) => env.nodeSocket(name),
  });

  const fresh = await env.restoreScenario(args.scenario);
  await env.start(fresh);
  if (watch && builder) watcher = watchNode(env, builder);

  const dashboardUrl = proxy && proxyPort === 443
    ? "https://localhost/"
    : `https://localhost:${proxy ? proxyPort : dashboardPort}/`;
  print(`\n  Dashboard: ${dashboardUrl}  <- Start here`);
  if (args.headless) return;
  print(
    "  If the browser warns about the certificate, run `mkcert -install` once.",
  );
  if (!process.stdin.isTTY) return;
  keys = listenKeys({
    open: () => openBrowser(dashboardUrl),
    restart: () => void env.restart(),
    quit: () => void quit(),
  });
  print("  Keys: space opens the dashboard, r restarts the nodes, q quits\n");
}

/** The flags (see above); exits if they are not these. */
function flags() {
  try {
    return parseArgs({
      args: process.argv.slice(2),
      options: {
        headless: { type: "boolean" },
        "no-watch": { type: "boolean" },
        "no-proxy": { type: "boolean" },
        scenario: { type: "string" },
        dassie: { type: "string" },
        "port-offset": { type: "string" },
        "state-dir": { type: "string" },
        xrpl: { type: "string" },
      },
      strict: true,
    }).values;
  } catch (error) {
    console.error((error as Error).message);
    process.exit(2);
  }
}

/** Builds the node binary; throws if it does not build. */
async function buildNode(env: Environment): Promise<CargoBuilder> {
  const builder = new CargoBuilder(
    () => env.buildStarted(),
    (_, line) => {
      if (/^(error|warning: unused)/.test(line)) print(`[cargo] ${line}`);
    },
  );
  print("building the node…");
  const result = await builder.build();
  env.buildFinished(result);
  if (!result.ok) throw new Error(`the node did not build:\n${result.excerpt}`);
  return builder;
}

/**
 * Builds the wallet and the dashboard into `dist-dev/`, emptied first: a
 * watch build keeps old files, so they pile up for one run at most.
 */
async function buildPages(env: Environment): Promise<void> {
  const viteLine = (app: string) => (_: unknown, line: string) => {
    if (/error|built in/i.test(line)) print(`[${app}] ${line}`);
  };
  const recorded = () => void env.recordPids();
  builds = ["wallet", "dev-dashboard"].map((app) =>
    new ViteBuild(`${ROOT}apps/${app}`, watch, viteLine(app), recorded)
  );
  for (const app of ["wallet", "dev-dashboard"]) {
    await rm(`${ROOT}apps/${app}/dist-dev`, { recursive: true, force: true })
      .catch(() => {});
  }
  print("building the wallet and the dashboard…");
  const results = await Promise.allSettled(
    builds.map((build) => build.start()),
  );
  for (const result of results) {
    if (result.status === "rejected") throw result.reason;
  }
}

/** Rebuilds the node and restarts the nodes when the Rust sources change. */
function watchNode(env: Environment, builder: CargoBuilder) {
  return watchSources([`${ROOT}crates`, `${ROOT}proto`], async (paths) => {
    try {
      env.log(LogLevel.INFO, `changed: ${paths.slice(0, 3).join(", ")}`);
      if (paths.some((path) => path.endsWith(".proto"))) await regenerate(env);
      const result = await builder.build();
      env.buildFinished(result);
      if (result.ok) await env.restart();
      else env.log(LogLevel.ERROR, "the node did not build");
    } catch (error) {
      env.log(LogLevel.ERROR, `rebuilding failed: ${error}`);
    }
  });
}

async function regenerate(env: Environment): Promise<void> {
  const { success } = await run(
    Deno.execPath(),
    ["run", "--no-lock", "-A", "scripts/gen.ts"],
    { cwd: `${ROOT}packages/rpc` },
  );
  env.log(
    success ? LogLevel.INFO : LogLevel.WARN,
    success
      ? "regenerated packages/rpc from proto/"
      : "could not regenerate packages/rpc: run `deno task gen` there",
  );
}
