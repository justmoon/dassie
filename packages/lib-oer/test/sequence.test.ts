import { describe, test } from "vitest"

import { boolean } from "../src/boolean"
import { uint32Number } from "../src/integer-as-number"
import { sequence } from "../src/sequence"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("sequence", () => {
  test("should be a function", ({ expect }) => {
    expect(sequence).toBeTypeOf("function")
  })

  describe("with empty schema", () => {
    const schema = sequence({})

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })
  })

  describe("with simple schema", () => {
    const schema = sequence({ first: boolean(), second: uint32Number() })

    test("should serialize a value", ({ expect }) => {
      const value = schema.serialize({ first: true, second: 0x12_34_56_78 })
      expect(value).toEqual(serializedOk("ff 12 34 56 78"))
    })

    test("should parse a value", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("ff 12 34 56 78"))
      expect(value).toEqual(parsedOk(5, { first: true, second: 0x12_34_56_78 }))
    })
  })
})
