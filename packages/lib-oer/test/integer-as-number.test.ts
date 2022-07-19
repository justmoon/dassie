import { describe, test } from "vitest"

import {
  INT8_MAX_NUMBER,
  INT8_MIN_NUMBER,
  UINT8_MAX_NUMBER,
  UINT_MIN_NUMBER,
  integerAsNumber,
} from "../src/integer-as-number"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("integerAsNumber", () => {
  test("should be a function", ({ expect }) => {
    expect(integerAsNumber).toBeTypeOf("function")
  })

  describe("schema 0..uint8_max", () => {
    const schema = integerAsNumber([UINT_MIN_NUMBER, UINT8_MAX_NUMBER])

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should parse a zero", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("00"))
      expect(value).toEqual(parsedOk(1, 0))
    })

    test("should serialize a zero", ({ expect }) => {
      const value = schema.serialize(0)
      expect(value).toEqual(serializedOk("00"))
    })

    test("should serialize a uint8", ({ expect }) => {
      const value = schema.serialize(0x12)
      expect(value).toEqual(serializedOk("12"))
    })
  })

  describe("schema int8_min..int8_max", () => {
    const schema = integerAsNumber([INT8_MIN_NUMBER, INT8_MAX_NUMBER])

    test("should serialize a negative int8", ({ expect }) => {
      const value = schema.serialize(-0x12)
      expect(value).toEqual(serializedOk("ee"))
    })
  })
})
