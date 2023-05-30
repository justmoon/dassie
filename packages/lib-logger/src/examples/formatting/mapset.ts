import { showComparison } from "./util/compare"

const mapset = {
  map: new Map([["foo", "bar"]]),
  set: new Set([["foo", "bar"]]),
  emptymap: new Map(),
  emptyset: new Set(),
  weakmap: new WeakMap(),
  weakset: new WeakSet(),
  objkey: new Map([[{}, "foo"]]),
  longmap: new Map([
    ["fafafafafafafafafafafa", 123_456],
    ["atatatatatatatatatatat", 456_789],
    ["bababababababababababa", 789_123],
  ]),
  longset: new Set([
    "long long long",
    "long long long 2",
    "long long long 3",
    "long long long 4",
    "long long long 5",
  ]),
}

showComparison(mapset, "Map and Set")
