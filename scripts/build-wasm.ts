// Builds the WASM modules of the core crates and their JavaScript bindings
// into packages/identity-wasm/gen/web: wasm-bindgen's `web` target, which
// browsers fetch and scripts and tests instantiate from the file.
//
// The modules are built with cargo and bound with the wasm-bindgen CLI, as
// Vorstand builds vault-core; the CLI's version must match the
// wasm-bindgen crate in Cargo.lock, which this script checks.

import { spawn } from "node:child_process";
import { readFile, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const output = `${root}packages/identity-wasm/gen`;
const crates = ["dassie-identity", "dassie-stream"];

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
for (const name of crates) {
  await run("cargo", [
    "rustc",
    "--package",
    name,
    "--lib",
    "--crate-type",
    "cdylib",
    "--target",
    "wasm32-unknown-unknown",
    "--profile",
    "wasm",
    "--features",
    "wasm",
  ]);
  const module = `${root}target/wasm32-unknown-unknown/wasm/${
    name.replaceAll("-", "_")
  }.wasm`;
  await run("wasm-bindgen", [
    "--target",
    "web",
    "--out-dir",
    `${output}/web`,
    module,
  ]);
}
console.log(`WASM bindings written to ${output}`);
