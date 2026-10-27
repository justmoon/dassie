// The phrase format of spec/decisions/phrase-format.md, written from the
// decision's text alone and on purpose not from `dassie-identity`, so the
// vectors it gives check that crate rather than repeat it: GF(2^11), the
// Reed–Solomon code over it, the 154 data bits, how a phrase is read, and
// the master seed. It uses no library but node:crypto (for HKDF-SHA256).
//
// The decoder is not the textbook one (syndromes, Berlekamp–Massey,
// Forney) but an exhaustive search: for each set of at most t places not
// known, it solves the syndrome equations for the values at those places
// and the erasures together, and takes every codeword it finds. The
// decision's bound leaves one check word spare, so at most one codeword can
// lie that close, and the search asserts it.

import { hkdfSync } from "node:crypto";

// ---- GF(2^11), primitive polynomial x^11 + x^2 + 1, α = x ----

export const FIELD_BITS = 11;
export const FIELD_SIZE = 1 << FIELD_BITS; // 2048
const ORDER = FIELD_SIZE - 1; // 2047, the order of the multiplicative group
const POLYNOMIAL = (1 << 11) | (1 << 2) | 1; // 0x805

const EXP = new Uint16Array(2 * ORDER);
const LOG = new Int32Array(FIELD_SIZE).fill(-1);
{
  let value = 1;
  for (let power = 0; power < ORDER; power += 1) {
    // α must be primitive: its powers reach every nonzero element once
    // before they come back to 1.
    if (LOG[value] !== -1) throw new Error(`x^11 + x^2 + 1 is not primitive: α^${power} repeats`);
    EXP[power] = value;
    LOG[value] = power;
    value <<= 1;
    if (value & FIELD_SIZE) value ^= POLYNOMIAL;
  }
  if (value !== 1) throw new Error("α^2047 is not 1");
  for (let power = ORDER; power < 2 * ORDER; power += 1) EXP[power] = EXP[power - ORDER];
}

export const add = (a, b) => a ^ b;
export const mul = (a, b) => (a === 0 || b === 0 ? 0 : EXP[LOG[a] + LOG[b]]);
export const inv = (a) => {
  if (a === 0) throw new Error("no inverse of 0");
  return EXP[(ORDER - LOG[a]) % ORDER];
};
export const pow = (a, n) => {
  if (a === 0) return n === 0 ? 1 : 0;
  return EXP[(LOG[a] * n) % ORDER];
};
export const alpha = (n) => EXP[((n % ORDER) + ORDER) % ORDER];

// ---- The code: 20 words, 14 data words, 6 check words ----

export const DATA_WORDS = 14;
export const CHECK_WORDS = 6;
export const WORDS = DATA_WORDS + CHECK_WORDS;

// g(x) = (x − α)(x − α²)…(x − α⁶), its coefficients highest first.
export const GENERATOR = (() => {
  let g = [1];
  for (let i = 1; i <= CHECK_WORDS; i += 1) {
    const root = alpha(i);
    const next = [...g, 0];
    for (let k = 1; k < next.length; k += 1) next[k] = add(next[k], mul(root, g[k - 1]));
    g = next;
  }
  return g;
})();

// The check words: the remainder of the data words, shifted up by six
// places, divided by g(x). The first word is the highest coefficient.
export const checkWords = (data) => {
  if (data.length !== DATA_WORDS) throw new Error("14 data words");
  const work = [...data, ...new Array(CHECK_WORDS).fill(0)];
  for (let i = 0; i < DATA_WORDS; i += 1) {
    const coefficient = work[i];
    if (coefficient === 0) continue;
    for (let j = 0; j < GENERATOR.length; j += 1) {
      work[i + j] = add(work[i + j], mul(GENERATOR[j], coefficient));
    }
  }
  if (work.slice(0, DATA_WORDS).some((word) => word !== 0)) throw new Error("division left data");
  return work.slice(DATA_WORDS);
};

// The word at place k is the coefficient of x^(19 − k); the syndrome S_j
// is the 20 words' polynomial at α^j.
const locator = (place) => alpha(WORDS - 1 - place);
export const syndromes = (words) =>
  Array.from({ length: CHECK_WORDS }, (_, index) => {
    const j = index + 1;
    let sum = 0;
    for (let place = 0; place < WORDS; place += 1) {
      sum = add(sum, mul(words[place], pow(locator(place), j)));
    }
    return sum;
  });

// Solves Σ_{k ∈ places} v_k · X_k^j = S_j for j = 1..6 by Gaussian
// elimination; the solution, or undefined if the equations disagree.
const solve = (places, syndrome) => {
  const columns = places.length;
  const rows = syndrome.map((value, index) => [
    ...places.map((place) => pow(locator(place), index + 1)),
    value,
  ]);
  let row = 0;
  for (let column = 0; column < columns; column += 1) {
    const pivot = rows.findIndex((candidate, index) => index >= row && candidate[column] !== 0);
    if (pivot === -1) throw new Error("distinct places give independent columns");
    [rows[row], rows[pivot]] = [rows[pivot], rows[row]];
    const scale = inv(rows[row][column]);
    rows[row] = rows[row].map((value) => mul(value, scale));
    for (let other = 0; other < rows.length; other += 1) {
      if (other === row || rows[other][column] === 0) continue;
      const factor = rows[other][column];
      rows[other] = rows[other].map((value, index) => add(value, mul(factor, rows[row][index])));
    }
    row += 1;
  }
  for (let rest = row; rest < rows.length; rest += 1) {
    if (rows[rest][columns] !== 0) return undefined;
  }
  return rows.slice(0, columns).map((solved) => solved[columns]);
};

const subsets = (items, most) => {
  const out = [[]];
  const grow = (start, current) => {
    if (current.length === most) return;
    for (let index = start; index < items.length; index += 1) {
      const next = [...current, items[index]];
      out.push(next);
      grow(index + 1, next);
    }
  };
  grow(0, []);
  return out;
};

// The bound of the decision's step 3: 2 × errors + erasures ≤ 5, and at
// most 2 errors at places not known.
export const MOST_UNKNOWN = 5;
export const errorBudget = (erasures) =>
  erasures > MOST_UNKNOWN ? -1 : Math.min(2, Math.floor((MOST_UNKNOWN - erasures) / 2));

// Restores 20 words, `undefined` at an erasure: the one codeword within the
// bound, or undefined if there is none.
export const decode = (received) => {
  if (received.length !== WORDS) throw new Error("20 words");
  const erased = [];
  const known = [];
  received.forEach((word, place) => (word === undefined ? erased : known).push(place));
  const budget = errorBudget(erased.length);
  if (budget < 0) return undefined;
  const filled = received.map((word) => word ?? 0);
  const syndrome = syndromes(filled);
  const found = new Map();
  for (const errors of subsets(known, budget)) {
    const places = [...erased, ...errors];
    const values = places.length === 0
      ? (syndrome.every((value) => value === 0) ? [] : undefined)
      : solve(places, syndrome);
    if (values === undefined) continue;
    const candidate = [...filled];
    places.forEach((place, index) => (candidate[place] = add(candidate[place], values[index])));
    if (syndromes(candidate).some((value) => value !== 0)) throw new Error("not a codeword");
    const wrong = known.filter((place) => candidate[place] !== received[place]).length;
    if (wrong > budget) throw new Error("outside the bound");
    found.set(candidate.join(","), candidate);
  }
  if (found.size > 1) throw new Error("two codewords within the bound");
  return found.values().next().value;
};

// ---- The 154 data bits: kind (6), created (20), secret (128) ----

export const KIND_BITS = 6;
export const CREATED_BITS = 20;
export const SECRET_BYTES = 16;
export const MOST_CREATED = (1 << CREATED_BITS) - 1;
export const KIND_REALMS = { 1: "test", 2: "live" };

export const pack = (kind, created, secret) => {
  if (!Number.isInteger(kind) || kind < 0 || kind >= 1 << KIND_BITS) throw new Error(`kind ${kind}`);
  if (!Number.isInteger(created) || created < 0 || created > MOST_CREATED) throw new Error(`created ${created}`);
  if (secret.length !== SECRET_BYTES) throw new Error("a 128-bit secret");
  let bits = (BigInt(kind) << 148n) | (BigInt(created) << 128n) | BigInt(`0x${Buffer.from(secret).toString("hex")}`);
  const data = [];
  for (let index = DATA_WORDS - 1; index >= 0; index -= 1) {
    data[index] = Number(bits & 0x7ffn);
    bits >>= 11n;
  }
  return data;
};

export const unpack = (data) => {
  let bits = 0n;
  for (const word of data) bits = (bits << 11n) | BigInt(word);
  const secret = Buffer.from((bits & ((1n << 128n) - 1n)).toString(16).padStart(32, "0"), "hex");
  return {
    kind: Number(bits >> 148n),
    created: Number((bits >> 128n) & BigInt(MOST_CREATED)),
    secret: new Uint8Array(secret),
  };
};

export const encode = (kind, created, secret) => {
  const data = pack(kind, created, secret);
  return { data, check: checkWords(data) };
};

// ---- Time: hours since 2026-01-01T00:00:00Z, rounded down ----

export const EPOCH_MS = Date.UTC(2026, 0, 1);
export const HOUR_MS = 3_600_000;
export const DAY_MS = 24 * HOUR_MS;
// The hour of a clock reading, or undefined outside the field.
export const hourOf = (ms) => {
  const hour = Math.floor((ms - EPOCH_MS) / HOUR_MS);
  return hour < 0 || hour > MOST_CREATED ? undefined : hour;
};
export const hourStart = (hour) => new Date(EPOCH_MS + hour * HOUR_MS).toISOString();
// The date of an hour, as a phrase's date is shown: `2026-02-19`.
export const dateOf = (hour) => hourStart(hour).slice(0, 10);

// ---- The master seed ----

export const masterSeed = (secret, realm) =>
  new Uint8Array(hkdfSync("sha256", secret, Buffer.from("dassie phrase"), Buffer.from(realm), 64));

// ---- Reading a phrase ----

const BIP39_LENGTHS = new Set([12, 15, 18, 21, 24]);

// A word's key: its first four letters, or all of it if it is shorter, in
// lower case. Only ASCII letters change case, as in dassie-identity: a
// letter whose Unicode lower case is an ASCII one (the Kelvin sign's is `k`)
// matches no word.
const key = (word) => word.slice(0, 4).replace(/[A-Z]/g, (letter) => letter.toLowerCase());

export const wordIndex = (wordlist) => {
  if (wordlist.length !== FIELD_SIZE) throw new Error("2048 words");
  const index = new Map();
  wordlist.forEach((word, value) => {
    if (index.has(key(word))) throw new Error(`two words share ${key(word)}`);
    index.set(key(word), value);
  });
  return index;
};

// Reads a phrase given as its words, `null` for a word the owner marks as
// unreadable, as a reader expecting `realm` whose clock reads `clock`
// (milliseconds since the Unix epoch). The outcome is what the decision's
// steps say, in their order.
export const read = (wordlist, input, { realm, clock }) => {
  const index = wordIndex(wordlist);
  if (input.length !== WORDS) {
    return { refused: BIP39_LENGTHS.has(input.length) ? "bip39" : "length" };
  }
  const received = input.map((word) => (word === null ? undefined : index.get(key(word))));
  const erasures = received.flatMap((word, place) => (word === undefined ? [place] : []));
  const restored = decode(received);
  if (restored === undefined) return { refused: "tooDamaged", erasures };
  const { kind, created, secret } = unpack(restored.slice(0, DATA_WORDS));
  if (kind === 0) return { refused: "kindZero" };
  const phraseRealm = KIND_REALMS[kind];
  if (phraseRealm === undefined) return { refused: "unassignedKind", kind };
  if (phraseRealm !== realm) return { refused: "otherRealm", realm: phraseRealm };
  // A creation time more than a day after the reader's clock, which the
  // refusal names by its date.
  if (EPOCH_MS + created * HOUR_MS - clock > DAY_MS) {
    return { refused: "createdInFuture", created, date: dateOf(created) };
  }
  const changed = restored.flatMap((word, place) => (word === received[place] ? [] : [place]));
  return {
    phrase: restored.map((word) => wordlist[word]).join(" "),
    corrected: changed.length > 0,
    changed,
    erasures,
    kind,
    realm: phraseRealm,
    created,
    date: dateOf(created),
    secret,
    masterSeed: masterSeed(secret, phraseRealm),
  };
};
