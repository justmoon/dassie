import { describe, test } from "vitest"

import {
  int8Bigint,
  int16Bigint,
  int32Bigint,
  int64Bigint,
  integerAsBigint,
  uint8Bigint,
  uint16Bigint,
  uint32Bigint,
  uint64Bigint,
} from "../src/integer-as-bigint"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import {
  getFixedLengthSamples,
  getVariableLengthSamples,
} from "./utils/sample-numbers"

describe("integerAsBigint", () => {
  test("should be a function", ({ expect }) => {
    expect(integerAsBigint).toBeTypeOf("function")
  })

  describe("schema variable-length signed integer", () => {
    const schema = integerAsBigint()

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should reject a zero-length zero", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("00"))
      expect(value).toMatchSnapshot()
    })

    test("should parse a zero", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("0100"))
      expect(value).toEqual(parsedOk(2, 0n))
    })

    test("should serialize a zero", ({ expect }) => {
      const value = schema.serialize(0n)
      expect(value).toEqual(serializedOk("0100"))
    })

    test("should parse a bigint with one byte", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("01 12"))
      expect(value).toEqual(parsedOk(2, 0x12n))
    })

    test("should serialize a bigint with one byte", ({ expect }) => {
      const value = schema.serialize(0x12n)
      expect(value).toEqual(serializedOk("01 12"))
    })

    test("should serialize a negative bigint with one byte", ({ expect }) => {
      const value = schema.serialize(-0x12n)
      expect(value).toEqual(serializedOk("01 ee"))
    })

    test("should serialize a very large number", ({ expect }) => {
      const testValue = 0x12_34_56_78_9a_bc_de_f0_12_34_56_78_9a_bc_de_f0n

      const value = schema.serialize(testValue)
      expect(value).toMatchSnapshot()
    })

    describe.each(getVariableLengthSamples(true))(
      "with sample number %s / %s",
      (value, hex) => {
        test("should parse the sample number", ({ expect }) => {
          const result = schema.parse(hexToUint8Array(hex))
          expect(result).toEqual(parsedOk(hex.length / 2, value))
        })

        test("should serialize the sample number", ({ expect }) => {
          const result = schema.serialize(value)
          expect(result).toEqual(serializedOk(hex))
        })
      }
    )
  })

  describe("schema variable-length unsigned integer", () => {
    const schema = integerAsBigint([0n, undefined])

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should reject a zero-length zero", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("00"))
      expect(value).toMatchSnapshot()
    })

    test("should parse a zero", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("0100"))
      expect(value).toEqual(parsedOk(2, 0n))
    })

    test("should serialize a zero", ({ expect }) => {
      const value = schema.serialize(0n)
      expect(value).toEqual(serializedOk("0100"))
    })

    test("should parse a bigint with one byte", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("01 12"))
      expect(value).toEqual(parsedOk(2, 0x12n))
    })

    test("should serialize a bigint with one byte", ({ expect }) => {
      const value = schema.serialize(0x12n)
      expect(value).toEqual(serializedOk("01 12"))
    })

    test("should refuse to serialize a negative bigint", ({ expect }) => {
      const value = schema.serialize(-0x12n)
      expect(value).toMatchSnapshot()
    })

    test("should serialize a very large number", ({ expect }) => {
      const testValue = 0x12_34_56_78_9a_bc_de_f0_12_34_56_78_9a_bc_de_f0n

      const value = schema.serialize(testValue)
      expect(value).toMatchSnapshot()
    })

    describe.each(getVariableLengthSamples(false))(
      "with sample number %s / %s",
      (value, hex) => {
        test("should parse the sample number", ({ expect }) => {
          const result = schema.parse(hexToUint8Array(hex))
          expect(result).toEqual(parsedOk(hex.length / 2, value))
        })

        test("should serialize the sample number", ({ expect }) => {
          const result = schema.serialize(value)
          expect(result).toEqual(serializedOk(hex))
        })
      }
    )
  })

  describe.each([
    ["uint8Bigint", uint8Bigint, false, 1],
    ["uint16Bigint", uint16Bigint, false, 2],
    ["uint32Bigint", uint32Bigint, false, 4],
    ["uint64Bigint", uint64Bigint, false, 8],
    ["int8Bigint", int8Bigint, true, 1],
    ["int16Bigint", int16Bigint, true, 2],
    ["int32Bigint", int32Bigint, true, 4],
    ["int64Bigint", int64Bigint, true, 8],
  ])("shortcut %s", (_, shortcutMethod, signed, byteLength) => {
    const schema = shortcutMethod()
    const sampleNumbers = getFixedLengthSamples(signed, byteLength)

    describe.each(sampleNumbers)("with sample number %s / %s", (value, hex) => {
      test("should parse the sample number", ({ expect }) => {
        const result = schema.parse(hexToUint8Array(hex))
        expect(result).toEqual(parsedOk(byteLength, value))
      })

      test("should serialize the sample number", ({ expect }) => {
        const result = schema.serialize(value)
        expect(result).toEqual(serializedOk(hex))
      })
    })
  })

  describe("schema uint8 with range 5..8", () => {
    const schema = integerAsBigint([5n, 8n])

    test("should parse a number in the range", ({ expect }) => {
      const result = schema.parse(hexToUint8Array("05"))
      expect(result).toEqual(parsedOk(1, 5n))
    })

    test("should serialize a number in the range", ({ expect }) => {
      const result = schema.serialize(5n)
      expect(result).toEqual(serializedOk("05"))
    })

    test("should not parse a number that is too high", ({ expect }) => {
      const result = schema.parse(hexToUint8Array("09"))
      expect(result).toMatchSnapshot()
    })

    test("should not serialize a number that is too high", ({ expect }) => {
      const result = schema.serialize(9n)
      expect(result).toMatchSnapshot()
    })

    test("should not parse a number that is too low", ({ expect }) => {
      const result = schema.parse(hexToUint8Array("01"))
      expect(result).toMatchSnapshot()
    })

    test("should not serialize a number that is too low", ({ expect }) => {
      const result = schema.serialize(1n)
      expect(result).toMatchSnapshot()
    })
  })
})
