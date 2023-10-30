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
        value: { type: "first", value: true },
        encoding: "80 ff",
      },
      {
        value: { type: "second", value: false },
        encoding: "81 00",
      },
      {
        value: { type: "third", value: true },
        encoding: "bf 88 00 ff",
      },
      {
        value: { type: "fourth", value: true },
        encoding: "bf ff 7f ff",
      },
      {
        value: { type: "fifth", value: false },
        encoding: "bf e0 2f 00",
      },
    ] as const

    test.each(testVectors)(
      "should serialize $encoding",
      ({ value, encoding }) => {
        const actual = schema.serialize(value)
        expect(actual).toEqual(serializedOk(encoding))
      },
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
        value: { type: "first", value: true },
        encoding: "80 ff",
      },
      {
        value: { type: "second", value: false },
        encoding: "81 00",
      },
      {
        value: { type: "third", value: true },
        encoding: "82 ff",
      },
      {
        value: { type: "fourth", value: true },
        encoding: "83 ff",
      },
      {
        value: { type: "fifth", value: false },
        encoding: "84 00",
      },
    ] as const

    test.each(testVectors)(
      "should serialize $encoding",
      ({ value, encoding }) => {
        const actual = schema.serialize(value)
        expect(actual).toEqual(serializedOk(encoding))
      },
    )

    test.each(testVectors)("should parse $encoding", ({ value, encoding }) => {
      const buffer = hexToUint8Array(encoding)
      const actual = schema.parse(buffer)
      expect(actual).toEqual(parsedOk(buffer.length, value))
    })
  })
})
