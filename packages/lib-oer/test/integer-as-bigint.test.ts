import { describe, test } from "vitest"

import { integerAsBigint } from "../src/integer-as-bigint"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"

describe("integerAsBigint", () => {
  test("should be a function", ({ expect }) => {
    expect(integerAsBigint).toBeTypeOf("function")
  })

  test("should return an object", ({ expect }) => {
    expect(integerAsBigint()).toBeTypeOf("object")
  })

  test("should parse a zero", ({ expect }) => {
    const value = integerAsBigint().parse(hexToUint8Array("00"))
    expect(value).toEqual(parsedOk(1, 0n))
  })

  test("should serialize a zero", ({ expect }) => {
    const value = integerAsBigint().serialize(0n)
    expect(value).toEqual(serializedOk("00"))
  })

  test("should parse a bigint with one byte", ({ expect }) => {
    const value = integerAsBigint().parse(hexToUint8Array("01 12"))
    expect(value).toEqual(parsedOk(2, 0x12n))
  })

  test("should serialize a bigint with one byte", ({ expect }) => {
    const value = integerAsBigint().serialize(0x12n)
    expect(value).toEqual(serializedOk("01 12"))
  })

  test("should serialize a negative bigint with one byte", ({ expect }) => {
    const value = integerAsBigint().serialize(-0x12n)
    expect(value).toEqual(serializedOk("01 ee"))
  })

  test("should serialize a very large number", ({ expect }) => {
    const testValue = 0x12_34_56_78_9a_bc_de_f0_12_34_56_78_9a_bc_de_f0n

    const value = integerAsBigint().serialize(testValue)
    expect(value).toMatchSnapshot()
  })
})
