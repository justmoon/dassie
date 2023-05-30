import { showComparison } from "./util/compare"

const object = { foo: "bar", baz: 123 }
const array = [1, 2, 3]
const map = new Map([["foo", "bar"]])
const set = new Set(["foo", "bar"])
const iterators = {
  generator: (function* () {
    // no-op
  })(),

  // These aren't iterators but included for completeness
  objentries: Object.entries(object),
  objkeys: Object.keys(object),
  objvalues: Object.values(object),

  arrayiter: array[Symbol.iterator](),
  arrayentries: array.entries(),

  mapiter: map[Symbol.iterator](),
  mapentries: map.entries(),
  mapkeys: map.keys(),
  mapvalues: map.values(),

  setiter: set[Symbol.iterator](),
  setentries: set.entries(),
  setkeys: set.keys(),
  setvalues: set.values(),
}

showComparison(iterators, "Iterators")
