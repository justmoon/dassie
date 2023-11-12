import { isFailure } from "@dassie/lib-type-utils"

import {
  parseBase128,
  predictBase128Length,
  serializeBase128,
} from "./base-128"
import { ParseFailure, SerializeFailure } from "./failures"
import { type ParseContext, isSafeUnsignedInteger } from "./parse"

export const TAG_MARKER_UNIVERSAL = 0b00
export const TAG_MARKER_APPLICATION = 0b01
export const TAG_MARKER_CONTEXT = 0b10
export const TAG_MARKER_PRIVATE = 0b11

export const tagClassMarkerMap = {
  universal: TAG_MARKER_UNIVERSAL,
  application: TAG_MARKER_APPLICATION,
  context: TAG_MARKER_CONTEXT,
  private: TAG_MARKER_PRIVATE,
} as const

export const tagMarkerClassMap = [
  "universal",
  "application",
  "context",
  "private",
] as const

export type TagClass = keyof typeof tagClassMarkerMap
export type TagMarker = (typeof tagClassMarkerMap)[TagClass]

export const parseTag = (
  context: ParseContext,
  offset: number,
):
  | ParseFailure
  | readonly [tag: number, tagClass: TagMarker, lengthOfTag: number] => {
  const { uint8Array } = context
  const tag = uint8Array[offset]

  if (tag == undefined) {
    return new ParseFailure(
      "unable to read tag - end of buffer",
      uint8Array,
      uint8Array.byteLength,
    )
  }

  const tagClassMarker = (tag >>> 6) as TagMarker

  const tagFirstByte = tag & 0b0011_1111

  // When tag is less than 63, it is encoded in the lower six bits of the first byte
  if (tagFirstByte !== 0b0011_1111) {
    return [tagFirstByte, tagClassMarker, 1] as const
  }

  // When tag is 63 or greater, it is encoded as a base-128 value following the first byte
  const base128ParseResult = parseBase128(
    context,
    offset + 1,
    Number.POSITIVE_INFINITY,
  )

  if (isFailure(base128ParseResult)) {
    return base128ParseResult
  }

  const [tagValue, lengthOfTag] = base128ParseResult

  return [Number(tagValue), tagClassMarker, lengthOfTag + 1] as const
}

export const serializeTag = (
  value: number,
  tagClass: TagMarker,
  uint8Array: Uint8Array,
  offset: number,
) => {
  if (!isSafeUnsignedInteger(value)) {
    return new SerializeFailure(
      "unable to serialize tag - value is out of bounds",
    )
  }

  if (value < 0b0011_1111) {
    uint8Array[offset] = value | (tagClass << 6)
    return
  }

  uint8Array[offset] = (tagClass << 6) | 0b0011_1111

  return serializeBase128(BigInt(value), uint8Array, offset + 1)
}

export const predictTagLength = (value: number) => {
  if (!isSafeUnsignedInteger(value)) {
    return new SerializeFailure(
      "unable to serialize tag - value is out of bounds",
    )
  }

  if (value < 0b0011_1111) {
    return 1
  }

  const lengthPredictionResult = predictBase128Length(BigInt(value))

  if (isFailure(lengthPredictionResult)) {
    return lengthPredictionResult
  }

  return lengthPredictionResult + 1
}
