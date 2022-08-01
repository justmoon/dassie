import { OerType } from "./base-type"
import { ParseError } from "./utils/errors"
import type { ParseContext, SerializeContext } from "./utils/parse"

export const boolean = () => {
  const OerBoolean = class extends OerType<boolean> {
    parseWithContext(
      { uint8Array, allowNoncanonical }: ParseContext,
      offset: number
    ) {
      const value = uint8Array[offset]
      if (typeof value === "undefined") {
        return new ParseError(
          "unable to read boolean value - end of buffer",
          uint8Array,
          offset
        )
      }
      if (!allowNoncanonical && value !== 0 && value !== 0xff) {
        return new ParseError(
          "unable to read boolean value - non-canonical",
          uint8Array,
          offset
        )
      }
      return [!!value, 1] as const
    }
    serializeWithContext(input: boolean) {
      return [
        ({ uint8Array }: SerializeContext, offset: number) => {
          uint8Array[offset] = input ? 0xff : 0
        },
        1,
      ] as const
    }
  }
  return new OerBoolean()
}
