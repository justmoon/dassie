/**
 * What the dev environment needs before it starts
 * (spec/areas/dev.md#prerequisites): a Unix, mkcert, and room for the nodes'
 * socket paths. Node.js and OpenSSL are no longer needed, and `dN.localhost`
 * need not resolve: the node's peer client maps it to loopback, and browsers do
 * the same.
 *
 * @module
 */

import process from "node:process";
import { checkSocketPath, type Layout } from "./paths.ts";
import { run } from "./process.ts";

/** What is missing, as messages for the developer; empty if nothing. */
export async function missingPrerequisites(
  layout: Layout,
  maxNodes: number,
): Promise<string[]> {
  const missing: string[] = [];
  if (process.platform !== "linux" && process.platform !== "darwin") {
    missing.push(
      "the dev environment runs on Linux and macOS; on Windows, use WSL, a container or a VM",
    );
  }
  try {
    const output = await run("mkcert", ["-version"]);
    if (!output.success) missing.push("mkcert does not run");
  } catch {
    missing.push(
      "mkcert is not installed: it is in `nix develop`, or see https://github.com/FiloSottile/mkcert",
    );
  }
  try {
    checkSocketPath(`${layout.sockets}/d${maxNodes}.sock`);
  } catch (error) {
    missing.push((error as Error).message);
  }
  return missing;
}
