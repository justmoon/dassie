import { describe, test } from "vitest"

import { hexToUint8Array } from "../src"
import { objectIdentifier } from "../src/object-identifier"
import { parsedOk, serializedOk } from "./utils/result"
import { getSampleObjectIdentifiers } from "./utils/sample-object-identifiers"
import { enableSnapshotSerializers } from "./utils/snapshot-serializers"

enableSnapshotSerializers()

describe("objectIdentifier", () => {
  test("should be a function", ({ expect }) => {
    expect(objectIdentifier).toBeTypeOf("function")
  })

  describe("with schema", () => {
    const schema = objectIdentifier()

    test("should return an object", ({ expect }) => {
      expect(schema).toBeTypeOf("object")
    })

    const objectIdentifiers = getSampleObjectIdentifiers()

    describe.each(objectIdentifiers)("with '%s' => %s", (value, hex) => {
      test("should parse a value", ({ expect }) => {
        const result = schema.parse(hexToUint8Array(hex))
        expect(result).toEqual(parsedOk(hex.length / 2, value))
      })

      test("should serialize a value", ({ expect }) => {
        const result = schema.serialize(value)
        expect(result).toEqual(serializedOk(hex))
      })
    })

    test("should refuse to parse an object identifier with a length prefix of 0", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("00"))
      expect(result).toMatchInlineSnapshot(`
        [ParseFailure(offset 0): object identifier of length zero is invalid

            00  
            ^^]
      `)
    })

    test("should refuse to serialize an object identifier with a first segment greater than 2", ({
      expect,
    }) => {
      const result = schema.serialize("3.0")
      expect(result).toMatchInlineSnapshot(
        "[SerializeFailure: object identifier first component must be in the range of 0..2]",
      )
    })

    test("should refuse to serialize an object identifier with a second segment greater than 39 when the first segment is 0", ({
      expect,
    }) => {
      const result = schema.serialize("1.40")
      expect(result).toMatchInlineSnapshot(
        "[SerializeFailure: object identifier second component must be in the range of 0..39 when first component is 0 or 1]",
      )
    })

    test("should refuse to serialize an object identifier with a second segment greater than 39 when the first segment is 1", ({
      expect,
    }) => {
      const result = schema.serialize("1.40")
      expect(result).toMatchInlineSnapshot(
        "[SerializeFailure: object identifier second component must be in the range of 0..39 when first component is 0 or 1]",
      )
    })

    test("should refuse to parse an object identifier with a length prefix of 3 that is only two bytes long", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("038837"))
      expect(result).toMatchInlineSnapshot(`
        [ParseFailure(offset 3): unable to read length prefix - end of buffer

            03 88 37  
                     ^^]
      `)
    })

    test("should refuse to parse an object identifier with a length prefix of 1 that is two bytes long", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("018837"))
      expect(result).toMatchInlineSnapshot(`
        [ParseFailure(offset 2): unable to read base-128 value - value is longer than expected based on context

            01 88 37  
                  ^^]
      `)
    })

    test("should refuse to parse an object identifier with unnecessary extra padding", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("03808837"))
      expect(result).toMatchInlineSnapshot(`
        [ParseFailure(offset 1): invalid base-128 value - must not contain unnecessary padding

            03 80 88 37  
               ^^]
      `)
    })
  })
})
