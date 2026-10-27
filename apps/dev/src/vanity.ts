/**
 * The dev nodes' identities (spec/areas/dev.md#dev-identities): the table
 * keeps each node's phrase secret and the node ID it gives in each realm;
 * the phrase, the node key and the login proof are derived from the secret
 * by `dassie-identity`, as the wallet derives them. A phrase names its
 * realm and the master seed takes it (spec/decisions/phrase-format.md), so
 * one secret is one node in the test realm and another in the live realm.
 * A node is named by its index, `d1` for the first, and the dashboard finds
 * a node's name from its ID through the table ({@linkcode names}).
 *
 * `scripts/vanity.ts` checks the table, and writes its IDs again from the
 * secrets when the form of a node ID changes.
 *
 * @module
 */

import { loginSecret, nodeId, phraseFromSecret, publicKey } from "@dassie/wasm";
import table from "../vanity.json" with { type: "json" };
import { MAX_NODES } from "./scenario/scenarios.ts";

/** Entries kept: one for each node a dev environment may run, d1 to d100. */
export const MAX_INDEX = MAX_NODES - 1;

/** A realm a node runs in, which its phrase names. */
export type Realm = "test" | "live";

/**
 * When every dev phrase was made: hour 0 of the phrase's field,
 * 2026-01-01T00:00Z, before any reader's clock, so no reader refuses one
 * as made in the future.
 */
export const CREATED_HOURS = 0;

/**
 * An entry: the phrase's 128-bit secret as hex, and the node ID it derives
 * in the test realm and in the live realm.
 */
export type Entry = readonly [secret: string, test: string, live: string];

/** The table, by index (`d1` is 0). */
export type Table = ReadonlyMap<number, Entry>;

/** A dev node's identity in one realm. */
export interface Identity {
  readonly index: number;
  /** The short name, `d1` for index 0. */
  readonly name: string;
  readonly realm: Realm;
  readonly id: string;
  /** Its 20-word phrase, of `realm`. */
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

/** A table from its JSON form, `{"<index>": [secret, testId, liveId]}`. */
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
    const [secret, test, live] = value;
    if (
      value.length !== 3 || !/^[0-9a-f]{32}$/.test(secret) ||
      !/^[a-z2-7]{39}$/.test(test) || !/^[a-z2-7]{39}$/.test(live)
    ) {
      throw new Error(`dev identities: entry ${key}`);
    }
    entries.set(index, [secret, test, live]);
  }
  return entries;
}

/** The table's JSON form: one entry a line, by index. */
export function formatTable(entries: Table): string {
  const lines = [...entries.keys()].sort((a, b) => a - b).map((index) => {
    const entry = entries.get(index)!;
    return `  ${JSON.stringify(`${index}`)}: ${JSON.stringify(entry)}`;
  });
  return `{\n${lines.join(",\n")}\n}\n`;
}

/** The phrase of `secret` in `realm`, made at {@link CREATED_HOURS}. */
function phraseOf(
  secret: string,
  realm: Realm,
): { phrase: string; key: Uint8Array } {
  const made = phraseFromSecret(
    Uint8Array.fromHex(secret),
    realm,
    CREATED_HOURS,
  );
  try {
    return { phrase: made.text, key: made.nodeKey(realm) };
  } finally {
    made.free();
  }
}

/** The node ID `secret` gives in `realm`. */
export function idOf(secret: string, realm: Realm): string {
  return nodeId(publicKey(phraseOf(secret, realm).key));
}

/** The ID an entry holds for `realm`. */
function idIn([, test, live]: Entry, realm: Realm): string {
  return realm === "live" ? live : test;
}

/**
 * Derives the identity of an entry in `realm`, whose secret must give the
 * entry's ID for that realm (asserted).
 */
export function derive(
  index: number,
  entry: Entry,
  realm: Realm = "test",
): Identity {
  const [secret] = entry;
  const id = idIn(entry, realm);
  const { phrase, key } = phraseOf(secret, realm);
  const pub = publicKey(key);
  const derived = nodeId(pub);
  const name = `d${index + 1}`;
  if (derived !== id) {
    throw new Error(
      `dev identity ${name}: ${secret} gives ${derived} in the ${realm} realm`,
    );
  }
  return {
    index,
    name,
    realm,
    id,
    phrase,
    key,
    publicKey: pub,
    loginProofHex: loginSecret(key).toHex(),
  };
}

/** The identity of the node at `index` in `realm`, from `table`. */
export function identity(
  table: Table,
  index: number,
  realm: Realm = "test",
): Identity {
  const entry = table.get(index);
  if (!entry) throw new Error(`no dev identity for d${index + 1}`);
  return derive(index, entry, realm);
}

/** Each node ID of `table`, in both realms, with the short name of its node. */
export function names(table: Table): ReadonlyMap<string, string> {
  return new Map(
    [...table].flatMap(([index, [, test, live]]) => [
      [test, `d${index + 1}`] as const,
      [live, `d${index + 1}`] as const,
    ]),
  );
}
