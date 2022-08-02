import type { ReadonlyTuple } from "type-fest"

import { OerType } from "./base-type"
import { ParseError, SerializeError } from "./utils/errors"
import {
  parseLengthPrefix,
  predictLengthPrefixLength,
  serializeLengthPrefix,
} from "./utils/length-prefix"
import type { BaseContext, ParseContext, SerializeContext } from "./utils/parse"

type InferBitstringValue<T extends number> = ReadonlyTuple<boolean, T>

interface BitstringOptions {
  variableLength?: boolean
}

export const parseBitstring = (
  context: ParseContext,
  offset: number,
  size: number
) => {
  const byteLength = Math.ceil(size / 8)
  const output = Array.from({ length: size })
  for (let byteIndex = 0; byteIndex < byteLength; byteIndex++) {
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    const byte = context.uint8Array[offset + byteIndex]!

    for (let bitIndex = 0; bitIndex < 8; bitIndex++) {
      if (byteIndex * 8 + bitIndex >= size) {
        break
      }
      const bit = (byte >> (7 - bitIndex)) & 1
      output[byteIndex * 8 + bitIndex] = bit === 1
    }
  }
  return output
}

export const serializeBitstring = (
  context: BaseContext,
  offset: number,
  input: InferBitstringValue<number>
) => {
  const { uint8Array } = context
  const inputAsArray = input as boolean[]
  const byteLength = Math.ceil(inputAsArray.length / 8)

  for (let byteIndex = 0; byteIndex < byteLength; byteIndex++) {
    let byte = 0
    for (let bitIndex = 0; bitIndex < 8; bitIndex++) {
      const index = byteIndex * 8 + bitIndex
      if (index >= inputAsArray.length) break
      byte |= inputAsArray[index] ? 1 << (7 - bitIndex) : 0
    }
    uint8Array[offset + byteIndex] = byte
  }
}

export class OerVariableBitstring<
  TBitDefinition extends number
> extends OerType<InferBitstringValue<TBitDefinition>> {
  constructor(readonly bits: TBitDefinition) {
    super()
  }

  parseWithContext(context: ParseContext, offset: number) {
    const { uint8Array } = context
    const result = parseLengthPrefix(context, offset)

    if (result instanceof ParseError) {
      return result
    }

    const [length, lengthOfLength] = result

    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    const unusedBits = uint8Array[offset + lengthOfLength]!

    if (unusedBits > 7) {
      return new ParseError(
        "unable to read bitstring - unused bits greater than 7",
        uint8Array,
        offset + 1
      )
    }

    const output = parseBitstring(
      context,
      offset + lengthOfLength + 1,
      (length - 1) * 8 - unusedBits
    )

    return [
      output as InferBitstringValue<TBitDefinition>,
      lengthOfLength + length,
    ] as const
  }

  serializeWithContext(input: ReadonlyTuple<boolean, TBitDefinition>) {
    const byteLength = Math.ceil(this.bits / 8)
    const unusedBits = 8 - (this.bits % 8)
    const length = byteLength + 1
    const lengthOfLength = predictLengthPrefixLength(length)
    if (lengthOfLength instanceof SerializeError) {
      return lengthOfLength
    }

    const serializer = (context: SerializeContext, offset: number) => {
      const { uint8Array } = context
      const lengthOfLength = serializeLengthPrefix(length, uint8Array, offset)

      if (lengthOfLength instanceof SerializeError) {
        return lengthOfLength
      }

      uint8Array[offset + lengthOfLength] = unusedBits
      serializeBitstring(context, offset + lengthOfLength + 1, input)
      return
    }
    serializer.size = lengthOfLength + length
    return serializer
  }
}

export class OerFixedBitstring<TBitDefinition extends number> extends OerType<
  InferBitstringValue<TBitDefinition>
> {
  constructor(readonly bits: TBitDefinition) {
    super()
  }

  parseWithContext(context: ParseContext, offset: number) {
    const byteLength = Math.ceil(this.bits / 8)

    if (context.uint8Array.length - offset < byteLength) {
      return new ParseError(
        "unable to read bitstring value - end of buffer",
        context.uint8Array,
        offset
      )
    }

    const output = parseBitstring(context, offset, this.bits)

    return [output as InferBitstringValue<TBitDefinition>, byteLength] as const
  }

  serializeWithContext(input: InferBitstringValue<TBitDefinition>) {
    const inputAsArray = input as boolean[]
    if (inputAsArray.length !== this.bits) {
      return new SerializeError(
        `unable to serialize bitstring - expected ${this.bits} bits, got ${inputAsArray.length}`
      )
    }

    const serializer = (context: SerializeContext, offset: number) => {
      serializeBitstring(context, offset, input)
    }
    serializer.size = Math.ceil(this.bits / 8)

    return serializer
  }
}

export const bitstring = <TBitDefinition extends number>(
  bits: TBitDefinition,
  { variableLength = false }: BitstringOptions = {}
) => {
  return variableLength
    ? new OerVariableBitstring<TBitDefinition>(bits)
    : new OerFixedBitstring<TBitDefinition>(bits)
}
