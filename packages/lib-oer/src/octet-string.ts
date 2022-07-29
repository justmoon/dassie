import { isUint8Array } from "node:util/types"

import { IntermediateSerializationResult, OerType } from "./base-type"
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

export class OerFixedOctetString extends OerType<
  Uint8Array,
  Uint8Array | IntermediateSerializationResult
> {
  constructor(readonly length: SafeUnsignedInteger) {
    super()
  }

  parseWithContext({ uint8Array }: ParseContext, offset: number) {
    return [
      uint8Array.slice(offset, offset + this.length),
      this.length,
    ] as const
  }

  serializeWithContext(value: Uint8Array | IntermediateSerializationResult) {
    const length = isUint8Array(value) ? value.length : value[1]
    if (length !== this.length) {
      return new SerializeError(
        `Expected octet string of length ${this.length}, but got ${length}`
      )
    }

    return [
      (context: SerializeContext, offset: number) => {
        if (isUint8Array(value)) {
          context.uint8Array.set(value, offset)
        } else {
          value[0](context, offset)
        }
      },
      this.length,
    ] as const
  }

  containing<T>(subType: OerType<T>) {
    return new OerOctetStringContaining<T>(this, subType)
  }
}

export class OerVariableOctetString extends OerType<
  Uint8Array,
  Uint8Array | IntermediateSerializationResult
> {
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

  serializeWithContext(input: Uint8Array | IntermediateSerializationResult) {
    const length = isUint8Array(input) ? input.length : input[1]
    const lengthOfLengthPrefix = predictLengthPrefixLength(length)

    if (lengthOfLengthPrefix instanceof SerializeError) {
      return lengthOfLengthPrefix
    }

    return [
      (context: SerializeContext, offset: number) => {
        const lengthOfLengthPrefix = serializeLengthPrefix(
          length,
          context.uint8Array,
          offset
        )

        if (lengthOfLengthPrefix instanceof SerializeError) {
          return lengthOfLengthPrefix
        }

        if (isUint8Array(input)) {
          context.uint8Array.set(input, offset + lengthOfLengthPrefix)
        } else {
          input[0](context, offset + lengthOfLengthPrefix)
        }
        return
      },
      lengthOfLengthPrefix + length,
    ] as const
  }

  containing<T>(subType: OerType<T>) {
    return new OerOctetStringContaining<T>(this, subType)
  }
}

export const OerOctetStringContaining = class<TSubtype> extends OerType<
  TSubtype,
  TSubtype | Uint8Array
> {
  constructor(
    readonly octetStringType: OerFixedOctetString | OerVariableOctetString,
    readonly subType: OerType<TSubtype>
  ) {
    super()
  }

  parseWithContext(context: ParseContext, offset: number) {
    const octetStringResult = this.octetStringType.parseWithContext(
      context,
      offset
    )

    if (octetStringResult instanceof ParseError) {
      return octetStringResult
    }

    const [parsedOctetString, octetStringOverallLength] = octetStringResult

    const subTypeResult = this.subType.parseWithContext(
      context,
      // We don't have a direct way to find out the length of the length prefix (if there is one). But we know that the subtype contents are at the end of the parsed data and we can simply take the end of the whole encoding and subtract the length of the subtype field.
      offset + octetStringOverallLength - parsedOctetString.length
    )

    if (subTypeResult instanceof ParseError) {
      return subTypeResult
    }

    const [subTypeValue] = subTypeResult

    return [subTypeValue, octetStringOverallLength] as const
  }

  serializeWithContext(value: TSubtype | Uint8Array) {
    if (isUint8Array(value)) {
      return this.octetStringType.serializeWithContext(value)
    }

    const subtypeSerializeResult = this.subType.serializeWithContext(value)

    if (subtypeSerializeResult instanceof SerializeError) {
      return subtypeSerializeResult
    }

    return this.octetStringType.serializeWithContext(subtypeSerializeResult)
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
