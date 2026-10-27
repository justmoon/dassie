/**
 * The dev nodes' identities (spec/areas/dev.md#dev-identities): the table
 * keeps each node's BIP-39 entropy and the node ID it gives; the phrase, the
 * node key and the login proof are derived from the entropy by
 * `dassie-identity`, as the node derives them. A node is named by its
 * index, `d1` for the first, and the dashboard finds a node's name from its
 * ID through the table ({@linkcode names}).
 *
 * `scripts/vanity.ts` checks the table, and writes its IDs again from the
 * entropy when the form of a node ID changes.
 *
 * @module
 */

import { loginSecret, nodeId, phraseFromSecret, publicKey } from "@dassie/wasm";
import table from "../vanity.json" with { type: "json" };
import { MAX_NODES } from "./scenario/scenarios.ts";

/** Entries kept: one for each node a dev environment may run, d1 to d100. */
export const MAX_INDEX = MAX_NODES - 1;

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
    if (
      !Number.isSafeInteger(index) || index < 0 || index > MAX_INDEX ||
      `${index}` !== key
    ) {
      throw new Error(`dev identities: index ${key}`);
    }
    const [entropy, id] = value;
    if (
      value.length !== 2 || !/^[0-9a-f]{32}$/.test(entropy) ||
      !/^[a-z2-7]{39}$/.test(id)
    ) {
      throw new Error(`dev identities: entry ${key}`);
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

/** The phrase of `entropy`, a test-realm phrase made at 2026-01-01T00:00Z. */
function phraseOf(entropy: string): { phrase: string; key: Uint8Array } {
  const made = phraseFromSecret(Uint8Array.fromHex(entropy), "test", 0);
  try {
    return { phrase: made.text, key: made.nodeKey("test") };
  } finally {
    made.free();
  }
}

/** The node ID `entropy` gives. */
export function idOf(entropy: string): string {
  return nodeId(publicKey(phraseOf(entropy).key));
}

/** Derives the identity of `entropy`, which must give `id` (asserted). */
export function derive(index: number, [entropy, id]: Entry): Identity {
  const { phrase, key } = phraseOf(entropy);
  const pub = publicKey(key);
  const derived = nodeId(pub);
  if (derived !== id) {
    throw new Error(`dev identity d${index + 1}: ${entropy} gives ${derived}`);
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
  if (!entry) throw new Error(`no dev identity for d${index + 1}`);
  return derive(index, entry);
}

/** Each node ID of `table`, with the short name of its node. */
export function names(table: Table): ReadonlyMap<string, string> {
  return new Map(
    [...table].map(([index, [, id]]) => [id, `d${index + 1}`] as const),
  );
}
