import { describe, test } from "vitest"

import { utf8String } from "../src/string"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("string", () => {
  describe("shortcut utf8String", () => {
    test("should be a function", ({ expect }) => {
      expect(utf8String).toBeTypeOf("function")
    })

    describe("with fixed length", () => {
      const schema = utf8String({ fixedLength: 13 })

      test("should return an object", ({ expect }) => {
        expect(schema).toBeTypeOf("object")
      })

      test("should serialize a valid value", ({ expect }) => {
        const value = schema.serialize("Hello, world!")
        expect(value).toEqual(
          serializedOk("48 65 6c 6c 6f 2c 20 77 6f 72 6c 64 21")
        )
      })

      test("should parse a valid value", ({ expect }) => {
        const value = schema.parse(
          hexToUint8Array("48 65 6c 6c 6f 2c 20 77 6f 72 6c 64 21")
        )
        expect(value).toEqual(parsedOk(13, "Hello, world!"))
      })
    })

    describe("with variable length", () => {
      const schema = utf8String()

      test("should return an object", ({ expect }) => {
        expect(schema).toBeTypeOf("object")
      })

      test("should serialize a valid value", ({ expect }) => {
        const value = schema.serialize("Hello, world!")
        expect(value).toEqual(
          serializedOk("0d 48 65 6c 6c 6f 2c 20 77 6f 72 6c 64 21")
        )
      })

      test("should parse a valid value", ({ expect }) => {
        const value = schema.parse(
          hexToUint8Array("0d 48 65 6c 6c 6f 2c 20 77 6f 72 6c 64 21")
        )
        expect(value).toEqual(parsedOk(14, "Hello, world!"))
      })
    })
  })
})
