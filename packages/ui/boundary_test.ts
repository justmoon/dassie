// The package is shared by the wallet and the dev dashboard, so it imports
// nothing of either: only itself, Solid, the RPC packages, and uqr, a QR
// encoder without dependencies of its own (for deposit addresses).

import { test } from "node:test";
import { assertEquals } from "@std/assert";
import { readdir, readFile } from "node:fs/promises";

const root = new URL(".", import.meta.url).pathname;

test("src imports only itself, Solid, the RPC packages and uqr", async () => {
  const allowed = [
    "solid-js",
    "@solidjs/web",
    "@dassie/rpc",
    "@dassie/rpc/proto",
    "uqr",
  ];
  const outside: string[] = [];
  const walk = async (dir: string) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory()) {
        await walk(path);
        continue;
      }
      if (!/\.tsx?$/.test(entry.name) || entry.name.includes("_test.")) {
        continue;
      }
      const code = await readFile(path, "utf8");
      for (const [, specifier] of code.matchAll(/from "([^"]+)"/g)) {
        if (allowed.includes(specifier)) continue;
        if (!specifier.startsWith(".")) {
          outside.push(`${path}: ${specifier}`);
          continue;
        }
        const target = new URL(specifier, `file://${path}`).pathname;
        if (!target.startsWith(`${root}src/`)) {
          outside.push(`${path}: ${specifier}`);
        }
      }
    }
  };
  await walk(`${root}src`);
  assertEquals(outside, []);
});
