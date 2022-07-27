import { OerType } from "./base-type"
import { ParseError, SerializeError } from "./utils/errors"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import {
  ParseContext,
  SafeUnsignedInteger,
  SerializeContext,
  isSafeUnsignedInteger,
} from "./utils/parse"
import { Range, parseRange } from "./utils/range"

export const OerFixedOctetString = class extends OerType<Uint8Array> {
  constructor(readonly length: SafeUnsignedInteger) {
    super()
  }

  parseWithContext({ uint8Array }: ParseContext, offset: number) {
    return [
      uint8Array.slice(offset, offset + this.length),
      this.length,
    ] as const
  }

  serializeWithContext(value: Uint8Array) {
    if (value.length !== this.length) {
      return new SerializeError(
        `Expected octet string of length ${this.length}, but got ${value.length}`
      )
    }

    return [
      (context: SerializeContext, offset: number) => {
        context.uint8Array.set(value, offset)
      },
      this.length,
    ] as const
  }
}

export const OerVariableOctetString = class extends OerType<Uint8Array> {
  constructor(
    readonly minimumLength?: SafeUnsignedInteger,
    readonly maximumLength?: SafeUnsignedInteger
  ) {
    super()
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context
    const result = parseLengthPrefix(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [length, lengthOfLength] = result

    return [
      uint8Array.slice(
        offset + lengthOfLength,
        offset + lengthOfLength + length
      ),
      lengthOfLength + length,
    ] as const
  }

  serializeWithContext(input: Uint8Array) {
    const length = input.length
    const lengthOfLengthPrefix = predictLengthPrefixLength(length)

    if (lengthOfLengthPrefix instanceof SerializeError) {
      return lengthOfLengthPrefix
    }

    return [
      ({ uint8Array }: SerializeContext, offset: number) => {
        const length = input.length
        const lengthOfLengthPrefix = serializeLengthPrefix(
          length,
          uint8Array,
          offset
        )

        if (lengthOfLengthPrefix instanceof SerializeError) {
          return lengthOfLengthPrefix
        }

        uint8Array.set(input, offset + lengthOfLengthPrefix)
        return
      },
      lengthOfLengthPrefix + length,
    ] as const
  }
}

export const octetString = (length?: Range<number>) => {
  const { minimum: minimumLength, maximum: maximumLength } = parseRange(length)
  if (minimumLength != undefined && !isSafeUnsignedInteger(minimumLength)) {
    throw new TypeError("minimumLength must be a safe unsigned integer")
  }
  if (maximumLength != undefined && !isSafeUnsignedInteger(maximumLength)) {
    throw new TypeError("maximumLength must be a safe unsigned integer")
  }

  if (
    minimumLength != undefined &&
    maximumLength != undefined &&
    minimumLength === maximumLength
  ) {
    return new OerFixedOctetString(minimumLength)
  }

  if (
    minimumLength != undefined &&
    maximumLength != undefined &&
    minimumLength > maximumLength
  ) {
    throw new TypeError(
      "minimumLength must be less than or equal to maximumLength"
    )
  }

  return new OerVariableOctetString(minimumLength, maximumLength)
}
