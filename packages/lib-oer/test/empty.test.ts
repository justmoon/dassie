import { describe, test } from "vitest"

import { empty } from "../src/empty"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

describe("empty", () => {
  test("should be a function", ({ expect }) => {
    expect(empty).toBeTypeOf("function")
  })

  describe("schema", () => {
    const schema = empty()

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should serialize a value of void", ({ expect }) => {
      const value = schema.serialize()
      expect(value).toEqual(serializedOk(""))
    })

    test("should parse a value of void", ({ expect }) => {
      const value = schema.parse(hexToUint8Array(""))
      expect(value).toEqual(parsedOk(0, undefined))
    })

    test("should refuse to parse a value with extra bytes at the end", ({
      expect,
    }) => {
      const value = schema.parse(hexToUint8Array("00"))
      expect(value).toMatchInlineSnapshot(`
        [ParseFailure(offset 0): non-canonical encoding - additional bytes present after the expected end of data

            00  
            ^^]
      `)
    })
  })
})
