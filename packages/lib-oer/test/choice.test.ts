import { describe, expect, test } from "vitest"

import { boolean } from "../src/boolean"
import { choice } from "../src/choice"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("choice", () => {
  test("should be a function", ({ expect }) => {
    expect(choice).toBeTypeOf("function")
  })

  describe("with explicit tags", () => {
    const schema = choice({
      first: boolean().tag(0),
      second: boolean().tag(1),
      third: boolean().tag(1024),
      fourth: boolean().tag(16_383),
      fifth: boolean().tag(12_335),
    })

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    const testVectors = [
      {
        value: { first: true },
        encoding: "80 ff",
      },
      {
        value: { second: false },
        encoding: "81 00",
      },
      {
        value: { third: true },
        encoding: "bf 88 00 ff",
      },
      {
        value: { fourth: true },
        encoding: "bf ff 7f ff",
      },
      {
        value: { fifth: false },
        encoding: "bf e0 2f 00",
      },
    ]

    test.each(testVectors)(
      "should serialize $encoding",
      ({ value, encoding }) => {
        const actual = schema.serialize(value)
        expect(actual).toEqual(serializedOk(encoding))
      }
    )

    test.each(testVectors)("should parse $encoding", ({ value, encoding }) => {
      const buffer = hexToUint8Array(encoding)
      const actual = schema.parse(buffer)
      expect(actual).toEqual(parsedOk(buffer.length, value))
    })
  })

  describe("with automatic tags", () => {
    const schema = choice({
      first: boolean(),
      second: boolean(),
      third: boolean(),
      fourth: boolean(),
      fifth: boolean(),
    })

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    const testVectors = [
      {
        value: { first: true },
        encoding: "80 ff",
      },
      {
        value: { second: false },
        encoding: "81 00",
      },
      {
        value: { third: true },
        encoding: "82 ff",
      },
      {
        value: { fourth: true },
        encoding: "83 ff",
      },
      {
        value: { fifth: false },
        encoding: "84 00",
      },
    ]

    test.each(testVectors)(
      "should serialize $encoding",
      ({ value, encoding }) => {
        const actual = schema.serialize(value)
        expect(actual).toEqual(serializedOk(encoding))
      }
    )

    test.each(testVectors)("should parse $encoding", ({ value, encoding }) => {
      const buffer = hexToUint8Array(encoding)
      const actual = schema.parse(buffer)
      expect(actual).toEqual(parsedOk(buffer.length, value))
    })
  })
})
