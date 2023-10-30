import { describe, test } from "vitest"

import { hexToUint8Array } from "../src"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "../src/utils/length-prefix"
import { getLengthPrefixSamples } from "./utils/sample-numbers"
import {
  createTestParseContext,
  createTestSerializeContext,
} from "./utils/test-context"

describe("length prefix", () => {
  describe.each(getLengthPrefixSamples())(
    "with sample length %s / %s",
    (value, hex) => {
      test("should parse correctly", ({ expect }) => {
        const context = createTestParseContext(hex)
        // We'll pretend that the array is actually as large as the length prefix would indicate
        Object.defineProperty(context.uint8Array, "length", {
          value: context.uint8Array.byteLength + value,
        })
        const result = parseLengthPrefix(context, 0)
        expect(result).toEqual([value, hex.length / 2])
      })

      test("should serialize correctly", ({ expect }) => {
        const context = createTestSerializeContext(hex)
        const result = serializeLengthPrefix(value, context.uint8Array, 0)

        expect(result).toEqual(hex.length / 2)
        expect(context.uint8Array).toEqual(hexToUint8Array(hex))
      })

      test("should predict length correctly", ({ expect }) => {
        const result = predictLengthPrefixLength(value)
        expect(result).toEqual(hex.length / 2)
      })
    },
  )
})
