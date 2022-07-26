import { describe, test } from "vitest"

import {
  INT8_MAX_NUMBER,
  INT8_MIN_NUMBER,
  UINT8_MAX_NUMBER,
  UINT_MIN_NUMBER,
  int8Number,
  int16Number,
  int32Number,
  integerAsNumber,
  uint8Number,
  uint16Number,
  uint32Number,
} from "../src/integer-as-number"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { getFixedLengthSamples } from "./utils/sample-numbers"

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

  describe.each([
    ["uint8Number", uint8Number, false, 1],
    ["uint16Number", uint16Number, false, 2],
    ["uint32Number", uint32Number, false, 4],
    ["int8Number", int8Number, true, 1],
    ["int16Number", int16Number, true, 2],
    ["int32Number", int32Number, true, 4],
  ])("shortcut %s", (_, shortcutMethod, signed, byteLength) => {
    const schema = shortcutMethod()
    const sampleNumbers = getFixedLengthSamples(signed, byteLength)

    describe.each(sampleNumbers)("with sample number %s / %s", (value, hex) => {
      test("should parse the sample number", ({ expect }) => {
        const result = schema.parse(hexToUint8Array(hex))
        expect(result).toEqual(parsedOk(byteLength, Number(value)))
      })

      test("should serialize the sample number", ({ expect }) => {
        const result = schema.serialize(Number(value))
        expect(result).toEqual(serializedOk(hex))
      })
    })
  })
})
