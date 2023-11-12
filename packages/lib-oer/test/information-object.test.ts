import { describe, test } from "vitest"

import {
  boolean,
  defineClass,
  defineObjectSet,
  hexToUint8Array,
  octetString,
  openType,
  sequence,
  uint8Number,
  uint16Number,
  uint32Number,
} from "../src"
import { parsedOk, serializedOk } from "./utils/result"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

const exampleClassDefinition = {
  type: uint8Number(),
  data: openType,
} as const

const exampleObjectSet = [
  {
    type: 1,
    data: octetString(),
  },
  {
    type: 2,
    data: uint32Number(),
  },
] as const

describe("information-object", () => {
  describe("defineClass", () => {
    test("should be a function", ({ expect }) => {
      expect(defineClass).toBeTypeOf("function")
    })

    test("should return an object", ({ expect }) => {
      const informationObjectClass = defineClass(exampleClassDefinition)

      expect(informationObjectClass).toBeTypeOf("object")
      expect(informationObjectClass).not.toBeNull()
    })
  })

  describe("defineObjectSet", () => {
    test("should be a function", ({ expect }) => {
      expect(defineObjectSet).toBeTypeOf("function")
    })

    test("should return an object", ({ expect }) => {
      const informationObjectClass = defineClass(exampleClassDefinition)
      const objectSet = defineObjectSet(
        informationObjectClass,
        exampleObjectSet,
      )

      expect(objectSet).toBeTypeOf("object")
      expect(objectSet).not.toBeNull()
    })
  })

  describe("sequence", () => {
    const informationObjectClass = defineClass(exampleClassDefinition)
    const objectSet = defineObjectSet(informationObjectClass, exampleObjectSet)
    const schema = sequence({
      first: boolean(),
      second: objectSet.type,
      third: objectSet.data,
    })

    test("should serialize a value of type 1", ({ expect }) => {
      const value = schema.serialize({
        first: true,
        second: 1,
        third: hexToUint8Array("fe fb fa"),
      })
      expect(value).toEqual(serializedOk("ff 01 04 03 fe fb fa"))
    })

    test("should serialize a value of type 2", ({ expect }) => {
      const value = schema.serialize({ first: true, second: 2, third: 8 })
      expect(value).toEqual(serializedOk("ff 02 04 00 00 00 08"))
    })

    test("should parse a value of type 1", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("ff 01 04 03 fe fb fa"))

      expect(value).toEqual(
        parsedOk(7, {
          first: true,
          second: 1,
          third: hexToUint8Array("fe fb fa"),
        }),
      )
    })

    test("should parse a value of type 2", ({ expect }) => {
      const value = schema.parse(hexToUint8Array("ff 02 04 00 00 00 08"))

      expect(value).toEqual(parsedOk(7, { first: true, second: 2, third: 8 }))
    })

    test("should allow extra bytes in open types when allowNoncanonical is true", ({
      expect,
    }) => {
      const value = schema.parse(
        hexToUint8Array("ff 02 05 01 02 03 04 00"),
        0,
        { allowNoncanonical: true },
      )

      expect(value).toEqual(
        parsedOk(8, {
          first: true,
          second: 2,
          third: 0x01_02_03_04,
        }),
      )
    })

    test("should not allow extra bytes in open types when allowNoncanonical is false", ({
      expect,
    }) => {
      const value = schema.parse(
        hexToUint8Array("ff 02 05 01 02 03 04 00"),
        0,
        { allowNoncanonical: false },
      )

      expect(value).toMatchInlineSnapshot(`
        [ParseFailure(offset 7): extra bytes inside of open type

            ff 02 05 01 02 03 04 00  
                                 ^^]
      `)
    })
  })

  describe("nested sequence", () => {
    const innerInformationObjectClass = defineClass(exampleClassDefinition)
    const innerObjectSet = defineObjectSet(
      innerInformationObjectClass,
      exampleObjectSet,
    )
    const innerSchema = sequence({
      first: boolean(),
      second: innerObjectSet.type,
      third: innerObjectSet.data,
    })
    const outerInformationObjectClass = defineClass({
      type: uint16Number(),
      data: openType,
    })
    const outerObjectSet = defineObjectSet(outerInformationObjectClass, [
      {
        type: 15_000,
        data: innerSchema,
      },
      {
        type: 15_001,
        data: uint32Number(),
      },
    ])
    const outerSchema = sequence({
      one: uint32Number(),
      two: outerObjectSet.type,
      three: outerObjectSet.data,
    })

    test("should serialize a value", ({ expect }) => {
      const value = outerSchema.serialize({
        one: 1,
        two: 15_000,
        three: {
          first: true,
          second: 1,
          third: hexToUint8Array("fe fb fa"),
        },
      })
      expect(value).toEqual(
        serializedOk("00 00 00 01 3a 98 07 ff 01 04 03 fe fb fa"),
      )
    })

    test("should parse a value", ({ expect }) => {
      const value = outerSchema.parse(
        hexToUint8Array("00 00 00 01 3a 98 07 ff 01 04 03 fe fb fa"),
      )

      expect(value).toEqual(
        parsedOk(14, {
          one: 1,
          two: 15_000,
          three: {
            first: true,
            second: 1,
            third: hexToUint8Array("fe fb fa"),
          },
        }),
      )
    })
  })
})
