import { describe, test } from "vitest"

import { boolean } from "../src/boolean"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

describe("boolean", () => {
  test("should be a function", ({ expect }) => {
    expect(boolean).toBeTypeOf("function")
  })

  describe("schema", () => {
    const schema = boolean()

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should serialize a value of true", ({ expect }) => {
      const value = schema.serialize(true)
      expect(value).toEqual(serializedOk("ff"))
    })

    test("should parse a value of true", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("ff"))
      expect(value).toEqual(parsedOk(1, true))
    })

    test("should serialize a value of false", ({ expect }) => {
      const value = schema.serialize(false)
      expect(value).toEqual(serializedOk("00"))
    })

    test("should parse a value of false", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("00"))
      expect(value).toEqual(parsedOk(1, false))
    })

    test("should refuse to parse a value with extra bytes at the end", ({
      expect,
    }) => {
      const value = schema.parse(hexToUint8Array("ff00"))
      expect(value).toMatchInlineSnapshot(`
        [ParseFailure(offset 1): non-canonical encoding - additional bytes present after the expected end of data

            ff 00  
               ^^]
      `)
    })
  })
})
