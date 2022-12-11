import { describe, test } from "vitest"

import { utf8String } from "../src/string"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { getLengthPrefixAsHex } from "./utils/sample-length-prefix"
import { utf8TestValues } from "./utils/sample-strings"

describe("string", () => {
  describe("shortcut utf8String", () => {
    test("should be a function", ({ expect }) => {
      expect(utf8String).toBeTypeOf("function")
    })

    describe("with fixed length 13", () => {
      const schema = utf8String(13)

      test("should return an object", ({ expect }) => {
        expect(schema).toBeTypeOf("object")
      })

      describe.each(utf8TestValues)("with '%s' => %s", (value, hex) => {
        const paddedValue = value.padEnd(13, " ")
        const paddedHex = hex.padEnd(
          hex.length + (paddedValue.length - value.length) * 2,
          "20"
        )
        const paddedHexWithLengthPrefix = `${getLengthPrefixAsHex(
          paddedHex.length / 2
        )}${paddedHex}`

        test("should parse", ({ expect }) => {
          const result = schema.parse(
            hexToUint8Array(paddedHexWithLengthPrefix)
          )
          expect(result).toEqual(
            parsedOk(paddedHexWithLengthPrefix.length / 2, paddedValue)
          )
        })

        test("should serialize", ({ expect }) => {
          const result = schema.serialize(paddedValue)
          expect(result).toEqual(serializedOk(paddedHexWithLengthPrefix))
        })
      })

      test("should refuse to parse a value of length 12", ({ expect }) => {
        const value = schema.parse(
          hexToUint8Array("0c313233343536373839303132")
        )
        expect(value).toMatchSnapshot()
      })

      test("should refuse to serialize a value of length 12", ({ expect }) => {
        const value = schema.serialize("123456789012")
        expect(value).toMatchSnapshot()
      })

      test("should refuse to parse a value of character length 12 but byte length of 13", ({
        expect,
      }) => {
        const value = schema.parse(
          hexToUint8Array("0d3132333435363738393031ceba")
        )
        expect(value).toMatchSnapshot()
      })

      test("should refuse to serialize a value of character length 12 but byte length of 13", ({
        expect,
      }) => {
        const value = schema.serialize("12345678901κ")
        expect(value).toMatchInlineSnapshot(`
          {
            "success": true,
            "value": Uint8Array [
              13,
              49,
              50,
              51,
              52,
              53,
              54,
              55,
              56,
              57,
              48,
              49,
              206,
              186,
            ],
          }
        `)
      })

      test("should refuse to parse a value of length 14", ({ expect }) => {
        const value = schema.parse(
          hexToUint8Array("0e3132333435363738393031323334")
        )
        expect(value).toMatchSnapshot()
      })

      test("should refuse to serialize a value of length 14", ({ expect }) => {
        const value = schema.serialize("1234567890123")
        expect(value).toMatchSnapshot()
      })
    })

    describe("with variable length", () => {
      const schema = utf8String()

      test("should return an object", ({ expect }) => {
        expect(schema).toBeTypeOf("object")
      })

      describe.each(utf8TestValues)("with '%s' => %s", (value, hex) => {
        const hexWithLengthPrefix = `${getLengthPrefixAsHex(
          hex.length / 2
        )}${hex}`

        test("should parse", ({ expect }) => {
          const result = schema.parse(hexToUint8Array(hexWithLengthPrefix))
          expect(result).toEqual(
            parsedOk(hexWithLengthPrefix.length / 2, value)
          )
        })

        test("should serialize", ({ expect }) => {
          const result = schema.serialize(value)
          expect(result).toEqual(serializedOk(hexWithLengthPrefix))
        })
      })
    })
  })
})
