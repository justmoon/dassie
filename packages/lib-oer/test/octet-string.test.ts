import { describe, test } from "vitest"

import { octetString } from "../src/octet-string"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { sampleBuffer } from "./utils/sample-buffer"

describe("octetString", () => {
  test("should be a function", ({ expect }) => {
    expect(octetString).toBeTypeOf("function")
  })

  describe("with fixed length", () => {
    const schema = octetString({ fixedLength: 22 })

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should serialize a valid value", ({ expect }) => {
      const value = schema.serialize(sampleBuffer.slice(0, 22))
      expect(value).toEqual(serializedOk(sampleBuffer.slice(0, 22)))
    })

    test("should parse a valid value", ({ expect }) => {
      const value = schema.parse(sampleBuffer.slice(0, 22))
      expect(value).toEqual(parsedOk(22, sampleBuffer.slice(0, 22)))
    })

    test("should refuse to serialize a value of the wrong length", ({
      expect,
    }) => {
      const value = schema.serialize(sampleBuffer.slice(0, 21))
      expect(value).toMatchSnapshot()
    })
  })

  describe("with variable length", () => {
    const schema = octetString()

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    test("should serialize an empty buffer", ({ expect }) => {
      const value = schema.serialize(new Uint8Array(0))
      expect(value).toEqual(serializedOk("00"))
    })

    test("should serialize a buffer with one byte", ({ expect }) => {
      const value = schema.serialize(hexToUint8Array("12"))
      expect(value).toEqual(serializedOk("01 12"))
    })

    test("should serialize a buffer with 128 bytes", ({ expect }) => {
      const value = schema.serialize(sampleBuffer.slice(0, 128))
      expect(value).toMatchSnapshot()
    })

    test("should reject a non-canonical length prefix which could fit one byte when allowNoncanonical is false", ({
      expect,
    }) => {
      const value = schema.parse(hexToUint8Array("81 01 12"))
      expect(value).toMatchSnapshot("error")
    })

    test("should accept a non-canonical length prefix which could fit one byte when allowNoncanonical is true", ({
      expect,
    }) => {
      const value = schema.parse(hexToUint8Array("81 01 12"), 0, {
        allowNoncanonical: true,
      })
      expect(value).toEqual(parsedOk(3, hexToUint8Array("12")))
    })

    test("should reject a non-canonical length prefix which could fit two bytes when allowNoncanonical is false", ({
      expect,
    }) => {
      const testVector = sampleBuffer.slice(0, 131)
      testVector.set(hexToUint8Array("82 00 80"), 0)
      const value = schema.parse(testVector)
      expect(value).toMatchSnapshot("error")
    })

    test("should reject a non-canonical length prefix which could fit two bytes when allowNoncanonical is true", ({
      expect,
    }) => {
      const testVector = sampleBuffer.slice(0, 131)
      testVector.set(hexToUint8Array("82 00 80"), 0)
      const value = schema.parse(testVector, 0, { allowNoncanonical: true })
      expect(value).toMatchSnapshot()
    })
  })
})
