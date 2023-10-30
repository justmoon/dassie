import { describe, test } from "vitest"

import { boolean } from "../src/boolean"
import { uint8Number, uint32Number } from "../src/integer-as-number"
import { sequence } from "../src/sequence"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("sequence", () => {
  test("should be a function", ({ expect }) => {
    expect(sequence).toBeTypeOf("function")
  })

  describe("empty schema", () => {
    const schema = sequence({})

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })
  })

  describe("simple schema", () => {
    const schema = sequence({ first: boolean(), second: uint32Number() })

    test("should serialize a value", ({ expect }) => {
      const value = schema.serialize({ first: true, second: 0x12_34_56_78 })
      expect(value).toEqual(serializedOk("ff 12 34 56 78"))
    })

    test("should parse a value", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("ff 12 34 56 78"))
      expect(value).toEqual(parsedOk(5, { first: true, second: 0x12_34_56_78 }))
    })
  })

  describe("schema with optional field", () => {
    const schema = sequence({
      first: boolean(),
      second: boolean().optional(),
      third: boolean(),
    })

    test("should parse a value where the optional field is not present", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("00FFFF"))
      expect(result).toEqual(parsedOk(3, { first: true, third: true }))
    })

    test("should serialize a value where the optional field is not present", ({
      expect,
    }) => {
      const result = schema.serialize({ first: true, third: true })
      expect(result).toEqual(serializedOk("00FFFF"))
    })

    test("should parse a value where the optional field is present", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("80FFFFFF"))
      expect(result).toEqual(
        parsedOk(4, { first: true, second: true, third: true }),
      )
    })

    test("should serialize a value where the optional field is present", ({
      expect,
    }) => {
      const result = schema.serialize({
        first: true,
        second: true,
        third: true,
      })
      expect(result).toEqual(serializedOk("80FFFFFF"))
    })
  })

  describe("schema containing field with default value", () => {
    const schema = sequence({
      first: uint8Number(),
      second: uint8Number().default(91),
      third: uint8Number(),
    })

    test("should parse and return the default value when the field is not present", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("000c0f"))
      expect(result).toEqual(parsedOk(3, { first: 12, second: 91, third: 15 }))
    })

    test("should serialize when the field is not present", ({ expect }) => {
      const result = schema.serialize({ first: 12, third: 15 })
      expect(result).toEqual(serializedOk("000c0f"))
    })

    test("should parse and return the given value when the field is present", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("800c480f"))
      expect(result).toEqual(parsedOk(4, { first: 12, second: 72, third: 15 }))
    })

    test("should serialize when the field is present", ({ expect }) => {
      const result = schema.serialize({ first: 12, second: 72, third: 15 })
      expect(result).toEqual(serializedOk("800c480f"))
    })

    test.skip("should serialize without the optional field if the value matches the default", ({
      expect,
    }) => {
      const result = schema.serialize({ first: 12, second: 91, third: 15 })
      expect(result).toEqual(serializedOk("000c0f"))
    })
  })

  describe("schema which is extensible but contains no extensions", () => {
    const schema = sequence({
      first: uint8Number(),
      second: uint8Number(),
    }).extensible()

    test("should parse a value", ({ expect }) => {
      const result = schema.parse(hexToUint8Array("000c0f"))
      expect(result).toEqual(parsedOk(3, { first: 12, second: 15 }))
    })

    test("should serialize a value", ({ expect }) => {
      const result = schema.serialize({ first: 12, second: 15 })
      expect(result).toEqual(serializedOk("000c0f"))
    })
  })

  describe("schema with a uint8 extension", () => {
    const schema = sequence({
      first: uint8Number(),
      second: uint8Number(),
    }).extend({
      ext1: uint8Number(),
    })

    test("should parse a value where the extension is not present", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("001234"))
      expect(result).toEqual(parsedOk(3, { first: 18, second: 52 }))
    })

    test("should serialize a value where the extension is not present", ({
      expect,
    }) => {
      const result = schema.serialize({ first: 18, second: 52 })

      expect(result).toEqual(serializedOk("001234"))
    })

    test("should parse a value where the extension is present", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("8012340207800156"))
      expect(result).toEqual(parsedOk(8, { first: 18, second: 52, ext1: 86 }))
    })

    test("should serialize a value where the extension is present", ({
      expect,
    }) => {
      const result = schema.serialize({ first: 18, second: 52, ext1: 86 })
      expect(result).toEqual(serializedOk("8012340207800156"))
    })
  })
})
