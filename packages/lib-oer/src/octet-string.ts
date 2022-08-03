import { isUint8Array } from "node:util/types"

import { OerType, Serializer } from "./base-type"
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
import { NormalizedRange, Range, parseRange } from "./utils/range"

export class OerFixedOctetString extends OerType<
  Uint8Array,
  Uint8Array | Serializer
> {
  constructor(readonly length: SafeUnsignedInteger) {
    super()
  }

  clone() {
    return new OerFixedOctetString(this.length)
  }

  parseWithContext({ uint8Array }: ParseContext, offset: number) {
    return [
      uint8Array.slice(offset, offset + this.length),
      this.length,
    ] as const
  }

  serializeWithContext(value: Uint8Array | Serializer) {
    const length = isUint8Array(value) ? value.length : value.size
    if (length !== this.length) {
      return new SerializeError(
        `Expected octet string of length ${this.length}, but got ${length}`
      )
    }

    const serializer = (context: SerializeContext, offset: number) => {
      if (isUint8Array(value)) {
        context.uint8Array.set(value, offset)
      } else {
        value(context, offset)
      }
    }
    serializer.size = this.length
    return serializer
  }

  containing<T>(subType: OerType<T>) {
    return new OerOctetStringContaining<T>(this, subType)
  }
}

export class OerVariableOctetString extends OerType<
  Uint8Array,
  Uint8Array | Serializer
> {
  constructor(readonly sizeRange: NormalizedRange<SafeUnsignedInteger>) {
    super()
  }

  clone() {
    return new OerVariableOctetString(this.sizeRange)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context
    const result = parseLengthPrefix(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [length, lengthOfLength] = result

    if (this.sizeRange[0] != undefined && length < this.sizeRange[0]) {
      return new ParseError(
        `Expected octet string of length at least ${this.sizeRange[0]}, but got ${length}`,
        uint8Array,
        offset
      )
    }

    if (this.sizeRange[1] != undefined && length > this.sizeRange[1]) {
      return new ParseError(
        `Expected octet string of length at most ${this.sizeRange[1]}, but got ${length}`,
        uint8Array,
        offset
      )
    }

    return [
      uint8Array.slice(
        offset + lengthOfLength,
        offset + lengthOfLength + length
      ),
      lengthOfLength + length,
    ] as const
  }

  serializeWithContext(input: Uint8Array | Serializer) {
    const length = isUint8Array(input) ? input.length : input.size

    if (this.sizeRange[0] != undefined && length < this.sizeRange[0]) {
      return new SerializeError(
        `Expected octet string of length at least ${this.sizeRange[0]}, but got ${length}`
      )
    }

    if (this.sizeRange[1] != undefined && length > this.sizeRange[1]) {
      return new SerializeError(
        `Expected octet string of length at most ${this.sizeRange[1]}, but got ${length}`
      )
    }

    const lengthOfLengthPrefix = predictLengthPrefixLength(length)

    if (lengthOfLengthPrefix instanceof SerializeError) {
      return lengthOfLengthPrefix
    }

    const serializer = (context: SerializeContext, offset: number) => {
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
        input(context, offset + lengthOfLengthPrefix)
      }
      return
    }
    serializer.size = lengthOfLengthPrefix + length
    return serializer
  }

  containing<T>(subType: OerType<T>) {
    return new OerOctetStringContaining<T>(this, subType)
  }
}

export class OerOctetStringContaining<TSubtype> extends OerType<
  TSubtype,
  TSubtype | Uint8Array
> {
  constructor(
    readonly octetStringType: OerFixedOctetString | OerVariableOctetString,
    readonly subType: OerType<TSubtype>
  ) {
    super()
  }

  clone() {
    return new OerOctetStringContaining(this.octetStringType, this.subType)
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
  const [minimumLength, maximumLength] = parseRange(length)
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

  return new OerVariableOctetString([minimumLength, maximumLength])
}
