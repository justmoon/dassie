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
 *   (also `DASSIE_DEV_STATE_DIR`).
 *
 * @module
 */

import { parseArgs } from "@std/cli/parse-args";
import { LogLevel } from "@dassie/rpc/proto";
import {
  CargoBuilder,
  DEV_BINARY,
  ViteBuild,
  watchSources,
} from "./src/build.ts";
import { caFile, dashboardCertificate } from "./src/certs.ts";
import { Environment } from "./src/environment.ts";
import { listenKeys, openBrowser } from "./src/keys.ts";
import { layout as makeLayout, ROOT } from "./src/paths.ts";
import { missingPrerequisites } from "./src/prereqs.ts";
import { type Proxy, proxyHint, startProxy } from "./src/proxy.ts";
import { MAX_NODES } from "./src/scenario/scenarios.ts";
import { startServer } from "./src/server.ts";
import { upstreamPort } from "./src/sni.ts";
import { stopStale } from "./src/stale.ts";

const DASHBOARD_PORT = 10100;
const CONTROL_PORT = 10101;
const PROXY_PORT = 443;

const args = parseArgs(Deno.args, {
  boolean: ["headless", "no-watch", "no-proxy"],
  string: ["scenario", "dassie", "port-offset", "state-dir"],
});
const offset = Number(args["port-offset"] ?? "0");
if (!Number.isSafeInteger(offset) || offset < 0 || offset > 40_000) {
  console.error("--port-offset must be a number from 0 to 40000");
  Deno.exit(2);
}
const print = (line: string) => console.log(line);
const layout = await makeLayout(
  args["state-dir"] ?? Deno.env.get("DASSIE_DEV_STATE_DIR"),
);
await Deno.mkdir(layout.state, { recursive: true });

print("Dassie//dev  Starting the development environment…");
const missing = await missingPrerequisites(layout, MAX_NODES);
if (missing.length > 0) {
  for (const line of missing) console.error(`  ✗ ${line}`);
  Deno.exit(1);
}
for (const stale of await stopStale(layout.pidsFile)) {
  print(`stopped ${stale}, left by an earlier run`);
}

await caFile(layout.tls);
const certificate = await dashboardCertificate(layout.tls);

// The proxy first: the wallet links depend on whether it runs.
let proxy: Proxy | undefined;
const proxyPort = PROXY_PORT + offset;
const dashboardPort = DASHBOARD_PORT + offset;
if (!args["no-proxy"]) {
  try {
    proxy = startProxy({
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
const environment = new Environment({
  layout,
  dassie: () => dassie,
  portOffset: offset,
  walletDir: `${ROOT}apps/wallet/dist-dev`,
  proxyPort: proxy ? proxyPort : undefined,
  debug: Deno.env.get("DEBUG") ?? "das:*",
  print,
});

// The node binary.
let builder: CargoBuilder | undefined;
if (args.dassie === undefined) {
  builder = new CargoBuilder(
    () => environment.buildStarted(),
    (_, line) => {
      if (/^(error|warning: unused)/.test(line)) print(`[cargo] ${line}`);
    },
  );
  print("building the node…");
  const result = await builder.build();
  environment.buildFinished(result);
  if (!result.ok) {
    console.error(result.excerpt);
    Deno.exit(1);
  }
}

// The wallet and the dashboard.
const watch = !args["no-watch"];
const viteLine = (app: string) => (_: unknown, line: string) => {
  if (/error|built in/i.test(line)) print(`[${app}] ${line}`);
};
const builds = [
  new ViteBuild(`${ROOT}apps/wallet`, watch, viteLine("wallet")),
  new ViteBuild(`${ROOT}apps/dev-dashboard`, watch, viteLine("dashboard")),
];
print("building the wallet and the dashboard…");
await Promise.all(builds.map((build) => build.start()));
environment.extraPids = () => builds.flatMap((build) => build.pids());

const server = startServer({
  ports: [dashboardPort, CONTROL_PORT + offset],
  certificatePem: certificate.certificatePem,
  keyPem: certificate.keyPem,
  dashboardDir: `${ROOT}apps/dev-dashboard/dist-dev`,
  control: environment.hub,
  nodeSocket: (name) => environment.nodeSocket(name),
});

await environment.restoreScenario(args.scenario);
await environment.start();

// Rebuild and restart the nodes when the Rust sources change.
const watcher = watch && builder
  ? watchSources([`${ROOT}crates`, `${ROOT}proto`], async (paths) => {
    environment.log(LogLevel.INFO, `changed: ${paths.slice(0, 3).join(", ")}`);
    if (paths.some((path) => path.endsWith(".proto"))) await regenerate();
    const result = await builder!.build();
    environment.buildFinished(result);
    if (result.ok) await environment.restart();
    else environment.log(LogLevel.ERROR, "the node did not build");
  })
  : undefined;

const dashboardUrl = proxy && proxyPort === 443
  ? "https://localhost/"
  : `https://localhost:${proxy ? proxyPort : dashboardPort}/`;
print(`\n  Dashboard: ${dashboardUrl}  <- Start here`);
if (!args.headless) {
  print(
    "  If the browser warns about the certificate, run `mkcert -install` once.",
  );
}

// Shutting down: once politely, a second time at once.
let quitting: Promise<void> | undefined;
const keys = !args.headless && Deno.stdin.isTerminal()
  ? listenKeys({
    open: () => openBrowser(dashboardUrl),
    restart: () => void environment.restart(),
    quit: () => void quit(),
  })
  : undefined;
if (keys) {
  print("  Keys: space opens the dashboard, r restarts the nodes, q quits\n");
}

function quit(): Promise<void> {
  if (quitting) {
    print("quitting at once");
    Deno.exit(130);
  }
  quitting = (async () => {
    print("stopping…");
    keys?.close();
    watcher?.close();
    await environment.stop();
    await Promise.all(builds.map((build) => build.stop()));
    proxy?.close();
    await server.close();
    await Deno.remove(layout.pidsFile).catch(() => {});
    Deno.exit(0);
  })();
  return quitting;
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  Deno.addSignalListener(signal, () => void quit());
}

async function regenerate(): Promise<void> {
  const { success } = await new Deno.Command(Deno.execPath(), {
    args: ["run", "--no-lock", "-A", "scripts/gen.ts"],
    cwd: `${ROOT}packages/rpc`,
    stdout: "null",
    stderr: "piped",
  }).output();
  environment.log(
    success ? LogLevel.INFO : LogLevel.WARN,
    success
      ? "regenerated packages/rpc from proto/"
      : "could not regenerate packages/rpc: run `deno task gen` there",
  );
}
