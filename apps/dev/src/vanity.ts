/**
 * The dev nodes' identities (spec/areas/dev.md#vanity-identities): node IDs
 * brute-forced to start with their short name, `d1_…` for the first. The table
 * keeps each one's BIP-39 entropy and ID; the phrase, the node key and the
 * login proof are derived from the entropy by `dassie-identity`, as the node
 * derives them.
 *
 * `scripts/vanity.ts` adds what a brute-forcer found, by the original's
 * rules ({@linkcode merge}).
 *
 * @module
 */

import {
  deriveNodeKey,
  loginSecret,
  mnemonicFromEntropy,
  nodeId,
  publicKey,
} from "@dassie/wasm";
import table from "../vanity.json" with { type: "json" };

/** Entries kept: d1 to d1000, and the record beyond. */
export const MAX_INDEX = 999;

/** An entry: the entropy as hex, and the node ID it derives. */
export type Entry = readonly [entropy: string, id: string];

/** The table, by index (`d1` is 0). */
export type Table = ReadonlyMap<number, Entry>;

/** A dev node's identity. */
export interface Identity {
  readonly index: number;
  /** The short name, `d1` for index 0. */
  readonly name: string;
  readonly id: string;
  readonly phrase: string;
  readonly key: Uint8Array;
  readonly publicKey: Uint8Array;
  /** The login proof, as the node's HTTP API takes it. */
  readonly loginProofHex: string;
}

/** The table in `vanity.json`. */
export function loadTable(): Table {
  return parseTable(table);
}

/** A table from its JSON form, `{"<index>": [entropy, id]}`. */
export function parseTable(json: Record<string, readonly string[]>): Table {
  const entries = new Map<number, Entry>();
  for (const [key, value] of Object.entries(json)) {
    const index = Number(key);
    if (!Number.isSafeInteger(index) || index < 0 || `${index}` !== key) {
      throw new Error(`vanity table: index ${key}`);
    }
    const [entropy, id] = value;
    if (value.length !== 2 || !/^[0-9a-f]{32}$/.test(entropy)) {
      throw new Error(`vanity table: entry ${key}`);
    }
    entries.set(index, [entropy, id]);
  }
  return entries;
}

/** The table's JSON form: one entry a line, by index. */
export function formatTable(entries: Table): string {
  const lines = [...entries.keys()].sort((a, b) => a - b).map((index) => {
    const [entropy, id] = entries.get(index)!;
    return `  ${JSON.stringify(`${index}`)}: ${JSON.stringify([entropy, id])}`;
  });
  return `{\n${lines.join(",\n")}\n}\n`;
}

/** The index a vanity ID is for: `d12_…` is 11. */
export function indexOf(id: string): number | undefined {
  const match = /^d([1-9]\d{0,8})_/.exec(id);
  return match ? Number(match[1]) - 1 : undefined;
}

/** Derives the identity of `entropy`, which must give `id` (asserted). */
export function derive(index: number, [entropy, id]: Entry): Identity {
  const phrase = mnemonicFromEntropy(Uint8Array.fromHex(entropy));
  const key = deriveNodeKey(phrase);
  const pub = publicKey(key);
  const derived = nodeId(pub);
  if (derived !== id) {
    throw new Error(`vanity entry d${index + 1}: ${entropy} gives ${derived}`);
  }
  return {
    index,
    name: `d${index + 1}`,
    id,
    phrase,
    key,
    publicKey: pub,
    loginProofHex: loginSecret(key).toHex(),
  };
}

/** The identity of the node at `index`, from `table`. */
export function identity(table: Table, index: number): Identity {
  const entry = table.get(index);
  if (!entry) throw new Error(`no vanity identity for d${index + 1}`);
  return derive(index, entry);
}

/** How much an ID's `-` and `_` spoil its look: fewer is better. */
function noise(id: string): number {
  return (id.match(/[-_]/g) ?? []).length;
}

/**
 * Adds `found` (entropy and ID, already checked to derive each other) to
 * `entries`, by the original's rules: an entry is kept if its index is
 * new, or its ID has fewer `-` and `_` than the one there. Indexes above
 * {@linkcode MAX_INDEX} are dropped, but the largest ever (the record).
 */
export function merge(entries: Table, found: readonly Entry[]): Table {
  const merged = new Map(entries);
  for (const entry of found) {
    const index = indexOf(entry[1]);
    if (index === undefined) throw new Error(`not a vanity ID: ${entry[1]}`);
    const known = merged.get(index);
    if (!known || noise(entry[1]) < noise(known[1])) merged.set(index, entry);
  }
  const record = Math.max(...merged.keys());
  for (const index of [...merged.keys()]) {
    if (index > MAX_INDEX && index !== record) merged.delete(index);
  }
  return merged;
}

/** A brute-forcer's line, `[0x]<entropy hex>: <node ID>`, or nothing. */
export function parseFound(line: string): Entry | undefined {
  const text = line.trim();
  if (text === "" || text.startsWith("#")) return undefined;
  const match = /^(?:0x)?([0-9a-fA-F]{32}):\s*(\S+)$/.exec(text);
  if (!match) throw new Error(`not an entry: ${text}`);
  return [match[1].toLowerCase(), match[2]];
}
