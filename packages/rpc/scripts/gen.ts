/**
 * Regenerates src/gen from proto/dassie/ui/v1 and proto/dassie/dev/v1 (with
 * buf and protoc-gen-es, per proto/buf.gen.yaml), and records each
 * contract's hash so that the drift test notices a contract change that was
 * not regenerated.
 *
 * Run from packages/rpc: `deno task gen` (needs the network once, for buf).
 */

import { spawn } from "node:child_process";
import { rm, writeFile } from "node:fs/promises";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { contractHash, CONTRACTS } from "./hash.ts";

const protoDir = new URL("../../../proto/", import.meta.url);
const genDir = new URL("../src/gen/", import.meta.url);

await rm(genDir, { recursive: true, force: true });
const buf = spawn(
  Deno.execPath(),
  ["run", "--no-lock", "-A", "npm:@bufbuild/buf@1.73.0", "generate"],
  { cwd: fileURLToPath(protoDir), stdio: "inherit" },
);
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
