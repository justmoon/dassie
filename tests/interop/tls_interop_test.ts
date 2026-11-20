// The node's TLS against OpenSSL's (Node's `tls`), in both directions,
// through the `tls_interop` example (crates/dassie-node/examples): with the
// hybrid key exchange X25519MLKEM768 alone on OpenSSL's side, and with
// X25519 alone, as a peer before the hybrid offers
// (spec/decisions/hybrid-key-exchange.md).

import { test } from "node:test";
import { type ChildProcess, spawn } from "node:child_process";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import type { AddressInfo } from "node:net";
import process from "node:process";
import tls from "node:tls";

import { assert, assertEquals } from "@std/assert";

const root = new URL("../../", import.meta.url).pathname;
const certificates = `${root}testdata/tls`;

/** Resolves with the exit code once `child` exited and its output ended. */
function exited(child: ChildProcess): Promise<number | null> {
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", resolve);
  });
}

/**
 * The example, built here, or the one `DASSIE_TLS_INTEROP` names: the Nix
 * check builds it beforehand, and runs without cargo.
 */
async function build(): Promise<string> {
  const built = process.env.DASSIE_TLS_INTEROP;
  if (built) return built;
  const code = await exited(
    spawn(
      "cargo",
      ["build", "--package", "dassie-node", "--example", "tls_interop"],
      { cwd: root, stdio: "inherit" },
    ),
  );
  assert(code === 0, "building the TLS interop example failed");
  return `${root}target/debug/examples/tls_interop`;
}

/** Runs the example with `args`; its lines once it exited. */
function run(binary: string, args: string[]) {
  const child = spawn(binary, args, { stdio: ["ignore", "pipe", "inherit"] });
  let output = "";
  child.stdout!.setEncoding("utf8");
  const firstLine = new Promise<string>((resolve) => {
    child.stdout!.on("data", (chunk: string) => {
      output += chunk;
      const end = output.indexOf("\n");
      if (end >= 0) resolve(output.slice(0, end));
    });
  });
  const lines = exited(child).then((code) => {
    assertEquals(code, 0, output);
    return output.trimEnd().split("\n");
  });
  return { firstLine, lines };
}

const binary = await build();
const cert = await readFile(`${certificates}/cert.pem`);
const key = await readFile(`${certificates}/key.pem`);
const ca = await readFile(`${certificates}/root.pem`);

for (const group of ["X25519MLKEM768", "X25519"]) {
  test(`the node's client and OpenSSL's server with ${group} alone`, async () => {
    const server = tls.createServer(
      { cert, key, ecdhCurve: group, minVersion: "TLSv1.3" },
      (socket) => socket.end("hello\n"),
    );
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    try {
      const { port } = server.address() as AddressInfo;
      const { lines } = run(binary, ["connect", String(port), certificates]);
      assertEquals(await lines, [group, "hello"]);
    } finally {
      server.close();
    }
  });

  test(`OpenSSL's client and the node's server with ${group} alone`, async () => {
    const { firstLine, lines } = run(binary, ["serve", certificates]);
    const port = Number(await firstLine);
    const socket = tls.connect({
      port,
      host: "127.0.0.1",
      servername: "localhost",
      ca,
      ecdhCurve: group,
      minVersion: "TLSv1.3",
    });
    let answer = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk: string) => (answer += chunk));
    await once(socket, "end");
    socket.destroy();
    assertEquals(answer, `${group}\n`);
    assertEquals(await lines, [String(port), group]);
  });
}
