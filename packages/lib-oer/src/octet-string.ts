import { isFailure } from "@dassie/lib-type-utils"

import { OerType, type Serializer } from "./base-type"
import { ParseFailure, SerializeFailure } from "./utils/failures"
import { isSerializer } from "./utils/is-serializer"
import { isUint8Array } from "./utils/is-uint8array"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import {
  type ParseContext,
  type SafeUnsignedInteger,
  type SerializeContext,
  isSafeUnsignedInteger,
} from "./utils/parse"
import { type NormalizedRange, type Range, parseRange } from "./utils/range"

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
    const length = isSerializer(value) ? value.size : value.length
    if (length !== this.length) {
      return new SerializeFailure(
        `Expected octet string of length ${this.length}, but got ${length}`,
      )
    }

    const serializer = (context: SerializeContext, offset: number) => {
      if (isSerializer(value)) {
        return value(context, offset)
      } else {
        context.uint8Array.set(value, offset)
      }
    }
    serializer.size = this.length
    return serializer
  }

  containing<TParseValue, TSerializeValue>(
    subType: OerType<TParseValue, TSerializeValue>,
  ) {
    return new OerOctetStringContaining(this, subType)
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
    if (isFailure(result)) return result

    const [length, lengthOfLength] = result

    if (this.sizeRange[0] != undefined && length < this.sizeRange[0]) {
      return new ParseFailure(
        `Expected octet string of length at least ${this.sizeRange[0]}, but got ${length}`,
        uint8Array,
        offset,
      )
    }

    if (this.sizeRange[1] != undefined && length > this.sizeRange[1]) {
      return new ParseFailure(
        `Expected octet string of length at most ${this.sizeRange[1]}, but got ${length}`,
        uint8Array,
        offset,
      )
    }

    return [
      uint8Array.slice(
        offset + lengthOfLength,
        offset + lengthOfLength + length,
      ),
      lengthOfLength + length,
    ] as const
  }

  serializeWithContext(input: Uint8Array | Serializer) {
    const length = isSerializer(input) ? input.size : input.length

    if (this.sizeRange[0] != undefined && length < this.sizeRange[0]) {
      return new SerializeFailure(
        `Expected octet string of length at least ${this.sizeRange[0]}, but got ${length}`,
      )
    }

    if (this.sizeRange[1] != undefined && length > this.sizeRange[1]) {
      return new SerializeFailure(
        `Expected octet string of length at most ${this.sizeRange[1]}, but got ${length}`,
      )
    }

    const lengthOfLengthPrefix = predictLengthPrefixLength(length)
    if (isFailure(lengthOfLengthPrefix)) return lengthOfLengthPrefix

    const serializer = (context: SerializeContext, offset: number) => {
      const lengthOfLengthPrefix = serializeLengthPrefix(
        length,
        context.uint8Array,
        offset,
      )
      if (isFailure(lengthOfLengthPrefix)) return lengthOfLengthPrefix

      if (isSerializer(input)) {
        return input(context, offset + lengthOfLengthPrefix)
      }

      context.uint8Array.set(input, offset + lengthOfLengthPrefix)
      return
    }
    serializer.size = lengthOfLengthPrefix + length
    return serializer
  }

  containing<TParseValue, TSerializeValue>(
    subType: OerType<TParseValue, TSerializeValue>,
  ) {
    return new OerOctetStringContaining(this, subType)
  }
}

export class OerOctetStringContaining<
  TParseValue,
  TSerializeValue,
> extends OerType<TParseValue, TSerializeValue | Uint8Array> {
  constructor(
    readonly octetStringType: OerFixedOctetString | OerVariableOctetString,
    readonly subType: OerType<TParseValue, TSerializeValue>,
  ) {
    super()
  }

  clone() {
    return new OerOctetStringContaining(this.octetStringType, this.subType)
  }

  parseWithContext(context: ParseContext, offset: number) {
    const octetStringResult = this.octetStringType.parseWithContext(
      context,
      offset,
    )
    if (isFailure(octetStringResult)) return octetStringResult

    const [parsedOctetString, octetStringOverallLength] = octetStringResult

    const subTypeResult = this.subType.parseWithContext(
      context,
      // We don't have a direct way to find out the length of the length prefix (if there is one). But we know that the subtype contents are at the end of the parsed data and we can simply take the end of the whole encoding and subtract the length of the subtype field.
      offset + octetStringOverallLength - parsedOctetString.length,
    )
    if (isFailure(subTypeResult)) return subTypeResult

    const [subTypeValue] = subTypeResult

    return [subTypeValue, octetStringOverallLength] as const
  }

  serializeWithContext(value: TSerializeValue | Uint8Array) {
    if (isUint8Array(value)) {
      return this.octetStringType.serializeWithContext(value)
    }

    const subtypeSerializeResult = this.subType.serializeWithContext(value)
    if (isFailure(subtypeSerializeResult)) return subtypeSerializeResult

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
      "minimumLength must be less than or equal to maximumLength",
    )
  }

  return new OerVariableOctetString([minimumLength, maximumLength])
}
