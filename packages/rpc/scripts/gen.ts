/**
 * Regenerates src/gen from proto/dassie/owner/v1 and proto/dassie/dev/v1 (with
 * buf and protoc-gen-es, per proto/buf.gen.yaml), and records each
 * contract's hash so that the drift test notices a contract change that was
 * not regenerated.
 *
 * Run from packages/rpc: `node --run gen`. buf and protoc-gen-es are the
 * ones installed here; both run in this Node.
 */

import { spawn } from "node:child_process";
import { rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { delimiter, dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { contractHash, CONTRACTS } from "./hash.ts";

const protoDir = new URL("../../../proto/", import.meta.url);
const genDir = new URL("../src/gen/", import.meta.url);

// buf's script, which runs the binary of this platform's package.
const bufBin = join(
  dirname(createRequire(import.meta.url).resolve("@bufbuild/buf/package.json")),
  "bin",
  "buf",
);

await rm(genDir, { recursive: true, force: true });
const buf = spawn(process.execPath, [bufBin, "generate"], {
  cwd: fileURLToPath(protoDir),
  stdio: "inherit",
  // buf.gen.yaml runs its plugin with `node`: this one.
  env: {
    ...process.env,
    PATH: `${dirname(process.execPath)}${delimiter}${process.env.PATH ?? ""}`,
  },
});
const code = await new Promise<number | null>((resolve, reject) => {
  buf.once("error", reject);
  buf.once("close", resolve);
});
if (code !== 0) process.exit(1);

for (const contract of CONTRACTS) {
  await writeFile(
    new URL(`${contract}/contract.sha256`, genDir),
    (await contractHash(contract)) + "\n",
  );
}
