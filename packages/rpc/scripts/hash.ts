/**
 * The SHA-256 of the contract's files, in name order, each as its name, a
 * newline, its length, a newline and its bytes, so that moving text between
 * files changes the hash.
 */

import { encodeHex } from "@std/encoding/hex";

const contractDir = new URL("../../../proto/dassie/ui/v1/", import.meta.url);

export async function contractHash(): Promise<string> {
  const names: string[] = [];
  for await (const entry of Deno.readDir(contractDir)) {
    if (entry.isFile && entry.name.endsWith(".proto")) names.push(entry.name);
  }
  names.sort();
  const parts: Uint8Array[] = [];
  const encoder = new TextEncoder();
  for (const name of names) {
    const bytes = await Deno.readFile(new URL(name, contractDir));
    parts.push(encoder.encode(`${name}\n${bytes.length}\n`), bytes);
  }
  const all = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    all.set(part, offset);
    offset += part.length;
  }
  return encodeHex(await crypto.subtle.digest("SHA-256", all));
}
