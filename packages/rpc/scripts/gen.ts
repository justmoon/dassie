/**
 * Regenerates src/gen from proto/dassie/ui/v1 (with buf and protoc-gen-es,
 * per proto/buf.gen.yaml), and records the contract's hash so that the
 * drift test notices a contract change that was not regenerated.
 *
 * Run from packages/rpc: `deno task gen` (needs the network once, for buf).
 */

import { contractHash } from "./hash.ts";

const protoDir = new URL("../../../proto/", import.meta.url);
const genDir = new URL("../src/gen/", import.meta.url);

await Deno.remove(genDir, { recursive: true }).catch(() => {});
const buf = new Deno.Command(Deno.execPath(), {
  args: ["run", "--no-lock", "-A", "npm:@bufbuild/buf@1.73.0", "generate"],
  cwd: protoDir,
  stdout: "inherit",
  stderr: "inherit",
});
const { success } = await buf.output();
if (!success) Deno.exit(1);

await Deno.writeTextFile(
  new URL("dassie/ui/v1/contract.sha256", genDir),
  (await contractHash()) + "\n",
);
