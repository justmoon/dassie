import { OerType } from "./base-type"
import { ParseError, SerializeError } from "./utils/errors"
import { parseLengthPrefix } from "./utils/length-prefix"
import type { ParseContext, SerializeContext } from "./utils/parse"

export class OerEnumerated<
  TEnumeration extends Record<string, number>
> extends OerType<keyof TEnumeration> {
  /**
   * A string containing the names of the enum options and their corresponding values, separated by commas. Used for debug messages.
   */
  private readonly setHint: string
  private readonly reverseMap: Record<number, keyof TEnumeration>

  constructor(readonly enumeration: TEnumeration) {
    super()

    this.reverseMap = Object.fromEntries(
      Object.entries(enumeration).map(([key, value]) => [value, key])
    ) as Record<number, keyof TEnumeration>

    this.setHint = Object.entries(enumeration)
      .map(([key, value]) => `${key}(${value})`)
      .join(",")
  }

  clone() {
    return new OerEnumerated(this.enumeration)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context
    const firstByte = uint8Array[offset]
    if (firstByte === undefined) {
      return new ParseError(
        "unable to read enumerated value - end of buffer",
        uint8Array,
        offset
      )
    }

    let numericEnumValue
    let length
    if (firstByte & 0x80) {
      const result = parseLengthPrefix(context, offset, true)
      if (result instanceof ParseError) {
        return result
      }
      ;[numericEnumValue, length] = result
    } else {
      numericEnumValue = firstByte
      length = 1
    }

    const enumValue = this.reverseMap[numericEnumValue]
    if (enumValue == undefined) {
      return new ParseError(
        `unable to read enumerated value - value ${firstByte} not in set ${this.setHint}`,
        uint8Array,
        offset
      )
    }

    return [enumValue, length] as const
  }

  serializeWithContext(input: string) {
    const value = this.enumeration[input]

    if (value == undefined) {
      return new SerializeError(
        `unable to serialize enumerated value - value ${input} not in set ${this.setHint}`
      )
    }

    const serializer = ({ uint8Array }: SerializeContext, offset: number) => {
      uint8Array[offset] = value
    }
    serializer.size = 1
    return serializer
  }
}

export const enumerated = <TEnumeration extends Record<string, number>>(
  enumeration: TEnumeration
) => {
  return new OerEnumerated(enumeration)
}
