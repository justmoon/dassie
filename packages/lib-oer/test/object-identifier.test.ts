import { describe, test } from "vitest"

import { hexToUint8Array } from "../src"
import { objectIdentifier } from "../src/object-identifier"
import { parsedOk, serializedOk } from "./utils/result"
import { getSampleObjectIdentifiers } from "./utils/sample-object-identifiers"

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

    test("should refuse to serialize an object identifier with a first segment greater than 2", ({
      expect,
    }) => {
      const result = schema.serialize("3.0")
      expect(result).toMatchSnapshot()
    })

    test("should refuse to serialize an object identifier with a second segment greater than 39 when the first segment is 0", ({
      expect,
    }) => {
      const result = schema.serialize("1.40")
      expect(result).toMatchSnapshot()
    })

    test("should refuse to serialize an object identifier with a second segment greater than 39 when the first segment is 1", ({
      expect,
    }) => {
      const result = schema.serialize("1.40")
      expect(result).toMatchSnapshot()
    })

    test("should refuse to parse an object identifier with a length prefix of 3 that is only two bytes long", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("038837"))
      expect(result).toMatchSnapshot()
    })

    test("should refuse to parse an object identifier with a length prefix of 1 that is two bytes long", ({
      expect,
    }) => {
      const result = schema.parse(hexToUint8Array("018837"))
      expect(result).toMatchSnapshot()
    })
  })
})
