/**
 * The SHA-256 of a contract's files, in name order, each as its name, a
 * newline, its length, a newline and its bytes, so that moving text between
 * files changes the hash.
 */

import { readdir, readFile } from "node:fs/promises";

/** The contracts generated here, as their directories under `proto/`. */
export const CONTRACTS = ["dassie/owner/v1", "dassie/dev/v1"] as const;
export type Contract = typeof CONTRACTS[number];

const protoDir = new URL("../../../proto/", import.meta.url);

export async function contractHash(contract: Contract): Promise<string> {
  const contractDir = new URL(`${contract}/`, protoDir);
  const names: string[] = [];
  for (const entry of await readdir(contractDir, { withFileTypes: true })) {
    if (entry.isFile() && entry.name.endsWith(".proto")) {
      names.push(entry.name);
    }
  }
  names.sort();
  const parts: Uint8Array[] = [];
  const encoder = new TextEncoder();
  for (const name of names) {
    const bytes = await readFile(new URL(name, contractDir));
    parts.push(encoder.encode(`${name}\n${bytes.length}\n`), bytes);
  }
  const all = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    all.set(part, offset);
    offset += part.length;
  }
  return new Uint8Array(await crypto.subtle.digest("SHA-256", all)).toHex();
}
