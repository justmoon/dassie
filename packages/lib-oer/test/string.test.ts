import { describe, test } from "vitest"

import { utf8String } from "../src/string"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { getLengthPrefixAsHex } from "./utils/sample-length-prefix"
import { utf8TestValues } from "./utils/sample-strings"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

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
          "20",
        )
        const paddedHexWithLengthPrefix = `${getLengthPrefixAsHex(
          paddedHex.length / 2,
        )}${paddedHex}`

        test("should parse", ({ expect }) => {
          const result = schema.parse(
            hexToUint8Array(paddedHexWithLengthPrefix),
          )
          expect(result).toEqual(
            parsedOk(paddedHexWithLengthPrefix.length / 2, paddedValue),
          )
        })

        test("should serialize", ({ expect }) => {
          const result = schema.serialize(paddedValue)
          expect(result).toEqual(serializedOk(paddedHexWithLengthPrefix))
        })
      })

      test("should refuse to parse a value of length 12", ({ expect }) => {
        const value = schema.parse(
          hexToUint8Array("0c313233343536373839303132"),
        )
        expect(value).toMatchInlineSnapshot(`
          [ParseFailure(offset 0): Expected octet string of length at least 13, but got 12

              0c 31 32 33 34 35 36 37 38 39 30 31 32  
              ^^]
        `)
      })

      test("should refuse to serialize a value of length 12", ({ expect }) => {
        const value = schema.serialize("123456789012")
        expect(value).toMatchInlineSnapshot(
          "[SerializeFailure: String is too short, expected at least 13 characters, got 12]",
        )
      })

      test("should refuse to parse a value of character length 12 but byte length of 13", ({
        expect,
      }) => {
        const value = schema.parse(
          hexToUint8Array("0d" + "3132333435363738393031ceba"),
        )
        expect(value).toMatchInlineSnapshot(`
          [ParseFailure(offset 0): String is too short, expected at least 13 characters, got 12

              0d 31 32 33 34 35 36 37 38 39 30 31 ce ba  
              ^^]
        `)
      })

      test("should refuse to serialize a value of character length 12 but byte length of 13", ({
        expect,
      }) => {
        const value = schema.serialize("12345678901κ")
        expect(value).toMatchInlineSnapshot(
          "[SerializeFailure: String is too short, expected at least 13 characters, got 12]",
        )
      })

      test("should refuse to parse a value of length 14", ({ expect }) => {
        const value = schema.parse(
          hexToUint8Array("0e3132333435363738393031323334"),
        )
        expect(value).toMatchInlineSnapshot(`
          [ParseFailure(offset 0): String is too long, expected at most 13 characters, got 14

              0e 31 32 33 34 35 36 37 38 39 30 31 32 33 34  
              ^^]
        `)
      })

      test("should refuse to serialize a value of length 14", ({ expect }) => {
        const value = schema.serialize("12345678901234")
        expect(value).toMatchInlineSnapshot(
          "[SerializeFailure: String is too long, expected at most 13 characters, got 14]",
        )
      })
    })

    describe("with variable length", () => {
      const schema = utf8String()

      test("should return an object", ({ expect }) => {
        expect(schema).toBeTypeOf("object")
      })

      describe.each(utf8TestValues)("with '%s' => %s", (value, hex) => {
        const hexWithLengthPrefix = `${getLengthPrefixAsHex(
          hex.length / 2,
        )}${hex}`

        test("should parse", ({ expect }) => {
          const result = schema.parse(hexToUint8Array(hexWithLengthPrefix))
          expect(result).toEqual(
            parsedOk(hexWithLengthPrefix.length / 2, value),
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
