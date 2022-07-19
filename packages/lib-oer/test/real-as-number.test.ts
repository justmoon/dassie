import { describe, test } from "vitest"

import { float64Number, realAsNumber } from "../src/real-as-number"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("realAsNumber", () => {
  test("should be a function", ({ expect }) => {
    expect(realAsNumber).toBeTypeOf("function")
  })

  describe("shortcut float64Number", () => {
    const schema = float64Number()

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should parse a zero", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("00 00 00 00 00 00 00 00"))
      expect(value).toEqual(parsedOk(8, 0))
    })

    test("should serialize a zero", ({ expect }) => {
      const value = schema.serialize(0)
      expect(value).toEqual(serializedOk("00 00 00 00 00 00 00 00"))
    })
  })
})
