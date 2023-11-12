import { isFailure } from "@dassie/lib-type-utils"

import { ParseFailure, SerializeFailure } from "./failures"
import type { ParseContext } from "./parse"

export const parseBase128 = (
  context: ParseContext,
  offset: number,
  endOffset: number,
): ParseFailure | readonly [value: bigint, lengthOfEncoding: number] => {
  const { uint8Array } = context

  if (uint8Array[offset] == 0x80) {
    return new ParseFailure(
      "invalid base-128 value - must not contain unnecessary padding",
      uint8Array,
      offset,
    )
  }

  // In OER base-128 encoding, the lower seven bits contain a value and the upper bit unset marks the end.
  let lengthOfEncoding = 0
  let value = 0n
  let nextByte: number | undefined
  do {
    if (offset + lengthOfEncoding >= endOffset) {
      return new ParseFailure(
        "unable to read base-128 value - value is longer than expected based on context",
        uint8Array,
        offset + lengthOfEncoding,
      )
    }

    nextByte = uint8Array[offset + lengthOfEncoding]

    if (nextByte == undefined) {
      return new ParseFailure(
        `unable to read base-128 value - end of buffer after ${lengthOfEncoding} bytes`,
        uint8Array,
        uint8Array.byteLength,
      )
    }

    value = (value << 7n) | BigInt(nextByte & 0b0111_1111)
    lengthOfEncoding++
  } while (nextByte & 0b1000_0000)

  return [value, lengthOfEncoding] as const
}

export const serializeBase128 = (
  value: bigint,
  uint8Array: Uint8Array,
  offset: number,
) => {
  const length = predictBase128Length(value)
  if (isFailure(length)) return length

  for (let index = length - 1; index >= 0; index--) {
    uint8Array[offset + index] =
      Number(value & 0b0111_1111n) | (index === length - 1 ? 0 : 0b1000_0000)
    value = value >> 7n
  }

  return length
}

export const predictBase128Length = (
  value: bigint,
): SerializeFailure | number => {
  if (value < 0n) {
    return new SerializeFailure(
      "unable to serialize base-128 - negative integers are not permitted",
    )
  }

  let length = 1,
    maxValue = 0b0111_1111n

  for (
    ;
    value > maxValue;
    length++, maxValue = (maxValue << 7n) | 0b0111_1111n
  ) {
    // empty
  }

  return length
}
