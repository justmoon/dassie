/**
 * `node --run cli -- dN <args…>` (spec 6 §6.9): the `dassie` CLI against a dev
 * node, through its IPC socket. For example `node --run cli -- d1 init`. For a
 * dev environment started with `--state-dir`, set `DASSIE_DEV_STATE_DIR`.
 *
 * @module
 */

import { spawn } from "node:child_process";
import process from "node:process";
import { DEV_BINARY } from "./src/build.ts";
import { layout, socketPath } from "./src/paths.ts";
import { exitCode } from "./src/process.ts";

const [name, ...args] = process.argv.slice(2);
if (!name || !/^d[1-9]\d{0,3}$/.test(name)) {
  console.error("usage: node --run cli -- dN <dassie arguments…>");
  process.exit(2);
}
const dassie = process.env.DASSIE_DEV_BINARY ?? DEV_BINARY;
const socket = socketPath(await layout(process.env.DASSIE_DEV_STATE_DIR), name);
const code = await new Promise<number>((resolve, reject) => {
  const child = spawn(dassie, args, {
    env: { ...process.env, DASSIE_IPC_SOCKET_PATH: socket },
    stdio: "inherit",
  });
  child.once("error", reject);
  child.once("close", (code, signal) => resolve(exitCode(code, signal)));
});
process.exit(code);
