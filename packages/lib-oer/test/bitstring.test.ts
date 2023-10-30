import { describe, test } from "vitest"

import { bitstring } from "../src/bitstring"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("bitstring", () => {
  test("should be a function", ({ expect }) => {
    expect(bitstring).toBeTypeOf("function")
  })

  test("should return an object", ({ expect }) => {
    expect(bitstring(0)).toBeTypeOf("object")
  })

  test("should serialize a fixed five bit value", ({ expect }) => {
    const value = bitstring(5).serialize([true, false, false, true, false])
    expect(value).toEqual(serializedOk("90"))
  })

  test("should parse a fixed five bit value", ({ expect }) => {
    const value = bitstring(5).parse(hexToUint8Array("90"))
    expect(value).toEqual(parsedOk(1, [true, false, false, true, false]))
  })

  test("should serialize a fixed eight bit value", ({ expect }) => {
    const value = bitstring(8).serialize([
      false,
      false,
      true,
      false,
      true,
      true,
      true,
      false,
    ])
    expect(value).toEqual(serializedOk("2e"))
  })

  test("should parse a fixed eight bit value", ({ expect }) => {
    const value = bitstring(8).parse(hexToUint8Array("2e"))
    expect(value).toEqual(
      parsedOk(1, [false, false, true, false, true, true, true, false]),
    )
  })

  test("should serialize a fixed thirteen bit value", ({ expect }) => {
    const value = bitstring(13).serialize([
      false,
      false,
      true,
      false,
      true,
      true,
      true,
      false,
      true,
      false,
      true,
      true,
      false,
    ])
    expect(value).toEqual(serializedOk("2e b0"))
  })

  test("should parse a fixed thirteen bit value", ({ expect }) => {
    const value = bitstring(13).parse(hexToUint8Array("2e b0"))
    expect(value).toEqual(
      parsedOk(2, [
        false,
        false,
        true,
        false,
        true,
        true,
        true,
        false,
        true,
        false,
        true,
        true,
        false,
      ]),
    )
  })

  test("should parse a variable length bitstring", ({ expect }) => {
    const value = bitstring(13, { variableLength: true }).parse(
      hexToUint8Array("03 03 cf b0"),
    )
    expect(value).toEqual(
      parsedOk(4, [
        true,
        true,
        false,
        false,
        true,
        true,
        true,
        true,
        true,
        false,
        true,
        true,
        false,
      ]),
    )
  })

  test("should serialize a variable length bitstring", ({ expect }) => {
    const value = bitstring(13, { variableLength: true }).serialize([
      true,
      true,
      false,
      false,
      true,
      true,
      true,
      true,
      true,
      false,
      true,
      true,
      false,
    ])

    expect(value).toEqual(serializedOk("03 03 cf b0"))
  })
})
