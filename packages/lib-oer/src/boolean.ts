import { OerType } from "./base-type"
import { ParseFailure } from "./utils/failures"
import type { ParseContext, SerializeContext } from "./utils/parse"

export class OerBoolean extends OerType<boolean> {
  clone() {
    return new OerBoolean()
  }

  parseWithContext(
    { uint8Array, allowNoncanonical }: ParseContext,
    offset: number,
  ) {
    const value = uint8Array[offset]
    if (value === undefined) {
      return new ParseFailure(
        "unable to read boolean value - end of buffer",
        uint8Array,
        offset,
      )
    }
    if (!allowNoncanonical && value !== 0 && value !== 0xff) {
      return new ParseFailure(
        "unable to read boolean value - non-canonical",
        uint8Array,
        offset,
      )
    }
    return [!!value, 1] as const
  }
  serializeWithContext(input: boolean) {
    const serializer = ({ uint8Array }: SerializeContext, offset: number) => {
      uint8Array[offset] = input ? 0xff : 0
    }
    serializer.size = 1
    return serializer
  }
}

export const boolean = () => {
  return new OerBoolean()
}
