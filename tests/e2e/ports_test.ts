/**
 * The end-to-end test files run in parallel, each starting its scenario or
 * dev environment at its own port offset (`PORT_OFFSETS` in harness.ts): no
 * two may take one port, or one fails to start with "Address already in
 * use". This checks the layout without starting anything.
 */

import { test } from "node:test";
import { assert } from "@std/assert";
import { readdir, readFile } from "node:fs/promises";
import {
  FAKE_LEDGER_PORT,
  FEED_PORT_BASE,
  HTTP_PORT_BASE,
  HTTPS_PORT_BASE,
  RATES_PORT,
  SCENARIOS,
} from "@dassie/dev/scenario";

import { FIXED_PORTS, PORT_OFFSETS } from "./harness.ts";

/**
 * The most nodes a file's scenario has, those added during a test
 * included: every scenario has fewer, leaving room for some added.
 */
const MAX_FILE_NODES = 16;
/** The ports the system hands out start here (Linux's default). */
const EPHEMERAL_PORTS = 32_768;

type Protocol = "tcp" | "udp";

/** The ports a file at `offset` takes: its nodes' and its fake ledger's,
 * faucet's and rate server's. */
function portsAt(offset: number): [Protocol, number][] {
  const ports: [Protocol, number][] = [];
  for (let n = 1; n <= MAX_FILE_NODES; n++) {
    ports.push(["tcp", HTTP_PORT_BASE + n + offset]);
    // HTTPS on TCP, QUIC on UDP.
    ports.push(["tcp", HTTPS_PORT_BASE + n + offset]);
    ports.push(["udp", HTTPS_PORT_BASE + n + offset]);
    ports.push(["udp", FEED_PORT_BASE + n + offset]);
  }
  ports.push(["tcp", FAKE_LEDGER_PORT + offset]);
  ports.push(["tcp", FAKE_LEDGER_PORT + 1 + offset]);
  ports.push(["tcp", RATES_PORT + offset]);
  return ports;
}

test("no two end-to-end test files take one port", () => {
  for (const def of SCENARIOS) {
    assert(
      def.schemes.length < MAX_FILE_NODES,
      `${def.id} has more nodes than a file's ports leave room for`,
    );
  }
  const taken = new Map<string, string>();
  const take = (who: string, protocol: Protocol, port: number) => {
    assert(port > 1024 && port < EPHEMERAL_PORTS, `${who} takes ${port}`);
    const key = `${protocol} ${port}`;
    const other = taken.get(key);
    assert(other === undefined, `${who} and ${other} both take ${key}`);
    taken.set(key, who);
  };
  for (const [file, offset] of Object.entries(PORT_OFFSETS)) {
    for (const [protocol, port] of portsAt(offset)) take(file, protocol, port);
  }
  for (const [who, protocol, port] of FIXED_PORTS) take(who, protocol, port);
});

// Every file that starts a scenario takes its offset from the table, so
// that the check above covers it.
test("every file that starts a scenario has an offset of its own", async () => {
  const dir = new URL(".", import.meta.url);
  for (const file of await readdir(dir)) {
    if (!file.endsWith("_test.ts") || file === "ports_test.ts") continue;
    const text = await readFile(new URL(file, dir), "utf8");
    if (!/Scenario\.temporary\(|portOffset/.test(text)) continue;
    assert(file in PORT_OFFSETS, `${file} has no port offset in harness.ts`);
    assert(
      text.includes(`PORT_OFFSETS["${file}"]`),
      `${file} does not take its port offset from PORT_OFFSETS`,
    );
  }
});

/**
 * How a port is spelled outside the table: in an address or URL, as an
 * option or a constant, or as a flag. A port a file named this way would
 * escape the check above.
 */
const PORT_LITERALS = [
  /(?:localhost|127\.0\.0\.1|\[::1?\]):\d/,
  /\bport:\s*[1-9]/i,
  /PORT\w*\s*=\s*\d/,
  /"--[\w-]*(?:port|listen)",\s*"[^"]*\d/,
  /waitForPort\(\d/,
];

// Every port a file names comes from harness.ts, so that the checks above
// cover it; a server that nothing needs to know the port of in advance
// listens on port 0, in the system's range, above every port here.
test("no test file names a port of its own", async () => {
  for (const dir of ["./", "../interop/"]) {
    const url = new URL(dir, import.meta.url);
    for (const file of await readdir(url)) {
      if (!file.endsWith(".ts")) continue;
      if (file === "harness.ts" || file === "ports_test.ts") continue;
      const text = await readFile(new URL(file, url), "utf8");
      for (const pattern of PORT_LITERALS) {
        const found = text.match(pattern);
        assert(
          found === null,
          `${dir}${file} names a port of its own (${
            found?.[0]
          }): take it from harness.ts`,
        );
      }
      // A standalone daemon binds its snapshot socket at 7768 on every
      // address unless told otherwise.
      if (text.includes('"--https-port"')) {
        assert(
          text.includes('"--feed-listen"'),
          `${dir}${file} starts a daemon without --feed-listen`,
        );
      }
      // Pebble's ports and the activated daemon's are one file's alone.
      if (file !== "fresh_install_test.ts") {
        assert(
          !/startPebble\(|startActivated\(/.test(text),
          `${dir}${file} starts Pebble or an activated daemon, whose ports are fresh_install_test.ts's`,
        );
      }
    }
  }
});
