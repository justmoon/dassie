import { describe, test } from "vitest"

import { OerType, boolean, captured, hexToUint8Array, sequence } from "../src"
import { parsedOk, serializedOk } from "./utils/result"

describe("captured", () => {
  test("should be a function", ({ expect }) => {
    expect(captured).toBeTypeOf("function")
  })

  describe("with boolean subtype", () => {
    const schema = captured(boolean())

    test("should be a OerType", ({ expect }) => {
      expect(schema).toBeInstanceOf(OerType)
    })

    test("should parse and capture bytes", ({ expect }) => {
      const result = schema.parse(hexToUint8Array("FF"))
      expect(result).toEqual(
        parsedOk(1, { value: true, bytes: hexToUint8Array("FF") }),
      )
    })

    test("should serialize when given value", ({ expect }) => {
      const result = schema.serialize({ value: true })
      expect(result).toEqual(serializedOk("FF"))
    })

    test("should serialize when given bytes", ({ expect }) => {
      const result = schema.serialize({ bytes: hexToUint8Array("FF") })
      expect(result).toEqual(serializedOk("FF"))
    })
  })

  describe("inside of a sequence", () => {
    const schema = sequence({
      a: boolean(),
      b: captured(boolean()),
      c: boolean(),
    })

    test("should parse and capture bytes", ({ expect }) => {
      const result = schema.parse(hexToUint8Array("FFFF00"))
      expect(result).toEqual(
        parsedOk(3, {
          a: true,
          b: {
            value: true,
            bytes: hexToUint8Array("FF"),
          },
          c: false,
        }),
      )
    })

    test("should serialize when given value", ({ expect }) => {
      const result = schema.serialize({ a: true, b: { value: true }, c: false })
      expect(result).toEqual(serializedOk("FFFF00"))
    })

    test("should serialize when given bytes", ({ expect }) => {
      const result = schema.serialize({
        a: true,
        b: { bytes: hexToUint8Array("FF") },
        c: false,
      })
      expect(result).toEqual(serializedOk("FFFF00"))
    })
  })
})
