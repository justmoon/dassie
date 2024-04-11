import { describe } from "vitest"

import { Trie } from "../src/trie/trie"

describe("Trie", (test) => {
  test("should insert the first value", ({ expect }) => {
    const trie = new Trie<number>()
    const result = trie.insert([], 1)

    expect(result).toBeUndefined()
    expect(trie.root).toMatchInlineSnapshot(`
      Map {
        "" => {
          "path": [],
          "value": 1,
        },
      }
    `)
  })

  test("should insert a value at a subpath", ({ expect }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b"], 1)
    const result2 = trie.insert(["a", "b", "c"], 2)

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(trie.root).toMatchInlineSnapshot(`
      Map {
        "a" => Map {
          "b" => Map {
            "" => {
              "path": [
                "a",
                "b",
              ],
              "value": 1,
            },
            "c" => {
              "path": [
                "a",
                "b",
                "c",
              ],
              "value": 2,
            },
          },
        },
      }
    `)
  })

  test("should insert a value at a superpath", ({ expect }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 2)
    const result2 = trie.insert(["a", "b"], 1)

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(trie.root).toMatchInlineSnapshot(`
      Map {
        "a" => Map {
          "b" => Map {
            "c" => {
              "path": [
                "a",
                "b",
                "c",
              ],
              "value": 2,
            },
            "" => {
              "path": [
                "a",
                "b",
              ],
              "value": 1,
            },
          },
        },
      }
    `)
  })

  test("should insert a value at a subpath with a common prefix", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "d"], 2)

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(trie.root).toMatchInlineSnapshot(`
      Map {
        "a" => Map {
          "b" => Map {
            "c" => {
              "path": [
                "a",
                "b",
                "c",
              ],
              "value": 1,
            },
            "d" => {
              "path": [
                "a",
                "b",
                "d",
              ],
              "value": 2,
            },
          },
        },
      }
    `)
  })

  test("should insert a value at a subpath with a common prefix and a wildcard", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "*"], 2)

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(trie.root).toMatchInlineSnapshot(`
      Map {
        "a" => Map {
          "b" => Map {
            "c" => {
              "path": [
                "a",
                "b",
                "c",
              ],
              "value": 1,
            },
            "*" => {
              "path": [
                "a",
                "b",
                "*",
              ],
              "value": 2,
            },
          },
        },
      }
    `)
  })

  test("should remove a value", ({ expect }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "d"], 2)

    trie.remove(["a", "b", "c"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(trie.root).toMatchInlineSnapshot(`
      Map {
        "a" => Map {
          "b" => {
            "path": [
              "a",
              "b",
              "d",
            ],
            "value": 2,
          },
        },
      }
    `)
  })

  test("should find a value", ({ expect }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "d"], 2)

    const resultF = trie.get(["a", "b", "c"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(resultF).toEqual(1)
  })

  test("should find a value with a segment wildcard at the end", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "?"], 2)

    const resultExact = trie.get(["a", "b", "c"])
    const resultWildcard = trie.get(["a", "b", "d"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(resultExact).toEqual(1)
    expect(resultWildcard).toEqual(2)
  })

  test("should find a value with a segment wildcard in the middle", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "?", "d"], 2)

    const resultExact = trie.get(["a", "b", "c"])
    const resultWildcard = trie.get(["a", "c", "d"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(resultExact).toEqual(1)
    expect(resultWildcard).toEqual(2)
  })

  test("should find a value with a prefix wildcard at the end", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "*"], 2)

    const resultExact = trie.get(["a", "b", "c"])
    const resultWildcardZero = trie.get(["a", "b"])
    const resultWildcardOne = trie.get(["a", "b", "d"])
    const resultWildcardMultiple = trie.get(["a", "b", "d", "e"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(resultExact).toEqual(1)
    expect(resultWildcardZero).toEqual(2)
    expect(resultWildcardOne).toEqual(2)
    expect(resultWildcardMultiple).toEqual(2)
  })

  test("should find most specific prefix wildcard route", ({ expect }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "c"], 1)
    const result2 = trie.insert(["a", "b", "*"], 2)
    const result3 = trie.insert(["a", "b", "c", "d", "e", "*"], 3)

    const resultExact = trie.get(["a", "b", "c"])
    const resultWildcardFirstZero = trie.get(["a", "b"])
    const resultWildcardFirstOne = trie.get(["a", "b", "d"])
    const resultWildcardFirstMultiple = trie.get(["a", "b", "c", "d"])
    const resultWildcardSecondZero = trie.get(["a", "b", "c", "d", "e"])
    const resultWildcardSecondOne = trie.get(["a", "b", "c", "d", "e", "f"])
    const resultWildcardSecondMultiple = trie.get([
      "a",
      "b",
      "c",
      "d",
      "e",
      "f",
      "g",
    ])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(result3).toBeUndefined()
    expect(resultExact).toEqual(1)
    expect(resultWildcardFirstZero).toEqual(2)
    expect(resultWildcardFirstOne).toEqual(2)
    expect(resultWildcardFirstMultiple).toEqual(2)
    expect(resultWildcardSecondZero).toEqual(3)
    expect(resultWildcardSecondOne).toEqual(3)
    expect(resultWildcardSecondMultiple).toEqual(3)
  })

  test("should prefer a segment over a prefix wildcard", ({ expect }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "?"], 1)
    const result2 = trie.insert(["a", "b", "*"], 2)

    const result = trie.get(["a", "b", "c"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(result).toEqual(1)
  })

  test("should prefer an exact prefix match over a segment wildcard", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result1 = trie.insert(["a", "b", "?"], 1)
    const result2 = trie.insert(["a", "b", "c", "*"], 2)

    const result = trie.get(["a", "b", "c"])

    expect(result1).toBeUndefined()
    expect(result2).toBeUndefined()
    expect(result).toEqual(2)
  })

  test("should not match an empty segment to a segment wildcard", ({
    expect,
  }) => {
    const trie = new Trie<number>()
    const result = trie.insert(["a", "b", "?"], 1)

    const resultWildcard = trie.get(["a", "b"])

    expect(result).toBeUndefined()
    expect(resultWildcard).toBeUndefined()
  })
})
