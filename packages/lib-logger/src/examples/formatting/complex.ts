import { showComparison } from "./util/compare"

const ARRAY_LENGTH = 2

const complex = {
  string: "Some String",
  longstring:
    "long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long long string",

  bool: true,

  int: 42,
  float: 3.14,
  posinf: Number.POSITIVE_INFINITY,
  neginf: Number.NEGATIVE_INFINITY,
  nan: Number.NaN,

  undef: undefined,
  null: null,

  symbol: Symbol("Example"),
  symfor: Symbol.for("Example for"),

  pojo: { foo: true },
  arr: [1, 2, 3],

  date: new Date(),
  regex: /example regexp matching thing/,
  err: new Error("Invalid stuff happened"),
  map: new Map([["foo", "bar"]]),
  set: new Set([["foo", "bar"]]),
  weakmap: new WeakMap(),
  weakset: new WeakSet(),
  uint8arr: new Uint8Array(ARRAY_LENGTH),
  promise: Promise.resolve(),
  proxy: new Proxy({}, {}),
  iterator: (function* () {
    // no-op
  })(),

  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  func: (_foo: number, _bar: string) => {
    // no-op
  },
  generator: function* () {
    // no-op
  },
  asyncfun: async function () {
    // no-op
  },
}

showComparison(complex, "Complex object")
