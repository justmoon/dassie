import { describe, test } from "vitest"

import { enumerated } from "../src/enumerated"
import { hexToUint8Array } from "../src/utils/hex"
import { parsedOk, serializedOk } from "./utils/result"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

describe("enumerated", () => {
  test("should be a function", ({ expect }) => {
    expect(enumerated).toBeTypeOf("function")
  })

  test("should return an object", ({ expect }) => {
    expect(enumerated({})).toBeTypeOf("object")
  })

  test("should serialize a value", ({ expect }) => {
    const value = enumerated({ red: 0, green: 1, blue: 2 }).serialize("blue")
    expect(value).toEqual(serializedOk("02"))
  })

  test("should parse a value", ({ expect }) => {
    const value = enumerated({ red: 0, green: 1, blue: 2 }).parse(
      hexToUint8Array("02"),
    )
    expect(value).toEqual(parsedOk(1, "blue"))
  })

  test("should fail to parse a value that is not in the set", ({ expect }) => {
    const value = enumerated({ red: 0, green: 1, blue: 2 }).parse(
      hexToUint8Array("03"),
    )
    expect(value).toMatchInlineSnapshot(
      `
      [ParseFailure(offset 0): unable to read enumerated value - value 3 not in set red(0),green(1),blue(2)

          03  
          ^^]
    `,
      "error",
    )
  })

  test("should fail to serialize a value that is not in the set", ({
    expect,
  }) => {
    const value = enumerated({ red: 0, green: 1, blue: 2 })
      // @ts-expect-error Intentionally passing a wrong value
      .serialize("yellow")
    expect(value).toMatchInlineSnapshot(
      "[SerializeFailure: unable to serialize enumerated value - value yellow not in set red(0),green(1),blue(2)]",
      "error",
    )
  })
})
