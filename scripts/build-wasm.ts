// Builds the WASM modules of the core crates and their JavaScript bindings
// into packages/identity-wasm/gen: `deno/` for Deno, `web/` for browsers.
//
// The modules are built with cargo and bound with the wasm-bindgen CLI, as
// Vorstand builds vault-core; the CLI's version must match the
// wasm-bindgen crate in Cargo.lock, which this script checks.

const root = new URL("..", import.meta.url).pathname;
const output = `${root}packages/identity-wasm/gen`;
const crates = ["dassie-identity", "dassie-stream"];

async function run(command: string, args: string[]): Promise<string> {
  const result = await new Deno.Command(command, {
    args,
    cwd: root,
    stdout: "piped",
    stderr: "inherit",
  }).output();
  if (!result.success) {
    throw new Error(`${command} ${args.join(" ")} failed`);
  }
  return new TextDecoder().decode(result.stdout);
}

async function lockedBindgenVersion(): Promise<string> {
  const lock = await Deno.readTextFile(`${root}Cargo.lock`);
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

await Deno.remove(output, { recursive: true }).catch(() => {});
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
  for (const target of ["deno", "web"]) {
    await run("wasm-bindgen", [
      "--target",
      target,
      "--out-dir",
      `${output}/${target}`,
      module,
    ]);
  }
}
console.log(`WASM bindings written to ${output}`);
