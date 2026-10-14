/**
 * `deno task cli dN <args…>` (spec 6 §6.9): the `dassie` CLI against a dev
 * node, through its IPC socket. For example `deno task cli d1 init`. For a
 * dev environment started with `--state-dir`, set `DASSIE_DEV_STATE_DIR`.
 *
 * @module
 */

import { DEV_BINARY } from "./src/build.ts";
import { layout, socketPath } from "./src/paths.ts";

const [name, ...args] = Deno.args;
if (!name || !/^d[1-9]\d{0,3}$/.test(name)) {
  console.error("usage: deno task cli dN <dassie arguments…>");
  Deno.exit(2);
}
const dassie = Deno.env.get("DASSIE_DEV_BINARY") ?? DEV_BINARY;
const { code } = await new Deno.Command(dassie, {
  args,
  env: {
    DASSIE_IPC_SOCKET_PATH: socketPath(
      await layout(Deno.env.get("DASSIE_DEV_STATE_DIR")),
      name,
    ),
  },
  stdin: "inherit",
  stdout: "inherit",
  stderr: "inherit",
}).output();
Deno.exit(code);
