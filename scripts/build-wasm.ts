// Builds the WASM modules of the core crates' bindings (dassie-wasm) and
// their JavaScript into packages/wasm/gen/web: wasm-bindgen's `web` target,
// which browsers fetch and scripts and tests instantiate from the file. Each
// of dassie-wasm's features is a module of its own, so the wallet loads
// identity without STREAM.
//
// The modules are built with cargo and bound with the wasm-bindgen CLI, as
// Vorstand builds vault-core; the CLI's version must match the
// wasm-bindgen crate in Cargo.lock, which this script checks.

import { spawn } from "node:child_process";
import { readFile, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const output = `${root}packages/wasm/gen`;
/** The bindings crate every module is built from. */
const crate = "dassie-wasm";
/** Each module: the feature of dassie-wasm it is built with, and its name. */
const modules = [
  { feature: "identity", name: "dassie_identity" },
  { feature: "stream", name: "dassie_stream" },
];

function run(command: string, args: string[]): Promise<string> {
  const child = spawn(command, args, {
    cwd: root,
    stdio: ["inherit", "pipe", "inherit"],
  });
  let stdout = "";
  child.stdout.setEncoding("utf8").on("data", (text: string) => {
    stdout += text;
  });
  return new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) => {
      if (code === 0) resolve(stdout);
      else reject(new Error(`${command} ${args.join(" ")} failed`));
    });
  });
}

async function lockedBindgenVersion(): Promise<string> {
  const lock = await readFile(`${root}Cargo.lock`, "utf8");
  const match = lock.match(
    /name = "wasm-bindgen"\nversion = "([^"]+)"/,
  );
  if (!match) throw new Error("wasm-bindgen is not in Cargo.lock");
  return match[1];
}

const locked = await lockedBindgenVersion();
const cli = (await run("wasm-bindgen", ["--version"])).trim();
if (cli !== `wasm-bindgen ${locked}`) {
  throw new Error(`${cli} does not match wasm-bindgen ${locked} in Cargo.lock`);
}

await rm(output, { recursive: true, force: true });
for (const { feature, name } of modules) {
  await run("cargo", [
    "rustc",
    "--package",
    crate,
    "--lib",
    "--crate-type",
    "cdylib",
    "--target",
    "wasm32-unknown-unknown",
    "--profile",
    "wasm",
    "--features",
    feature,
  ]);
  // Each build writes the same file, so it is bound before the next.
  await run("wasm-bindgen", [
    "--target",
    "web",
    "--out-dir",
    `${output}/web`,
    "--out-name",
    name,
    `${root}target/wasm32-unknown-unknown/wasm/${
      crate.replaceAll("-", "_")
    }.wasm`,
  ]);
}
console.log(`WASM bindings written to ${output}`);
