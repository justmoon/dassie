import { describe, test } from "vitest"

import { boolean } from "../src/boolean"
import { sequenceOf } from "../src/sequence-of"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk } from "./utils/result"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

describe("sequenceOf", () => {
  test("should be a function", ({ expect }) => {
    expect(sequenceOf).toBeTypeOf("function")
  })

  describe("schema boolean[]", () => {
    const schema = sequenceOf(boolean())

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should serialize a value", ({ expect }) => {
      const value = schema.serialize([true, false, true, true, false, false])
      expect(value).toMatchInlineSnapshot(
        "Uint8Array [ 01 06 ff 00 ff ff 00 00 ]",
      )
    })

    test("should parse a value", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("01 06 ff 00 ff ff 00 00"))
      expect(value).toEqual(
        parsedOk(8, [true, false, true, true, false, false]),
      )
    })
  })
})
