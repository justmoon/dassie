import { describe, test } from "vitest"

import { boolean, hexToUint8Array, sequence, uint8Number } from "../src"
import { objectIdentifier } from "../src/object-identifier"
import { constantTests } from "./utils/full-samples"
import { parsedOk, serializedOk } from "./utils/result"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

describe("constant", () => {
  describe.each(constantTests)("of %s", (_, baseType, value, hex) => {
    const schema = baseType.constant(value)

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should parse the correct value", ({ expect }) => {
      const result = schema.parse(hexToUint8Array(hex))
      expect(result).toEqual(parsedOk(hex.length / 2, value))
    })

    test("should serialize a valid value", ({ expect }) => {
      const value = schema.serialize()
      expect(value).toEqual(serializedOk(hex))
    })

    test("should fail to parse a value extended by one byte", ({ expect }) => {
      const result = schema.parse(hexToUint8Array(`${hex}00`))
      expect(result).toMatchSnapshot()
    })

    if (hex.length >= 2) {
      test("should fail to parse a value truncated by one byte", ({
        expect,
      }) => {
        const result = schema.parse(hexToUint8Array(hex.slice(0, -2)))
        expect(result).toMatchSnapshot()
      })

      test("should fail to parse a value with a different encoding than expected", ({
        expect,
      }) => {
        const result = schema.parse(
          hexToUint8Array(
            ((Number.parseInt(hex.slice(0, 2), 16) + 1) % 256)
              .toString(16)
              .padStart(2, "0") + hex.slice(2),
          ),
        )
        expect(result).toMatchSnapshot()
      })
    }
  })

  describe("inside of a sequence", () => {
    const schema = sequence({
      a: uint8Number(),
      b: objectIdentifier().constant("2.999"),
      c: boolean(),
    })

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should parse a valid value", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("01 02 88 37 FF"))
      expect(value).toEqual(parsedOk(5, { a: 1, b: "2.999", c: true }))
    })

    test("should serialize a valid value", ({ expect }) => {
      // Note that we do not need to provide the constant value during serialization.
      const value = schema.serialize({ a: 1, c: true })
      expect(value).toEqual(serializedOk("01 02 88 37 FF"))
    })
  })
})
