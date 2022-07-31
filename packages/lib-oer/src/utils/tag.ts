import { ParseError, SerializeError } from "./errors"
import { ParseContext, isSafeUnsignedInteger } from "./parse"

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
export type TagMarker = typeof tagClassMarkerMap[TagClass]

export const parseTag = (
  context: ParseContext,
  offset: number
):
  | ParseError
  | readonly [tag: number, tagClass: TagMarker, lengthOfTag: number] => {
  const { uint8Array } = context
  const tag = uint8Array[offset]

  if (tag == undefined) {
    return new ParseError(
      "unable to read tag - end of buffer",
      uint8Array,
      uint8Array.byteLength
    )
  }

  const tagClassMarker = (tag >>> 6) as TagMarker

  const tagFirstByte = tag & 0b0011_1111

  // When tag is less than 63, it is encoded in the lower six bits of the first byte
  if (tagFirstByte !== 0b0011_1111) {
    return [tagFirstByte, tagClassMarker, 1] as const
  }

  // When tag is 63 or greater, the first byte's lower six bits are set and the remaining bytes encode the tag in their lower seven bits until a byte with its upper bit set marks the end.
  let lengthOfTag = 1
  let tagValue = 0
  let nextByte: number | undefined
  do {
    nextByte = uint8Array[offset + lengthOfTag]

    if (nextByte == undefined) {
      return new ParseError(
        "unable to read tag - end of buffer",
        uint8Array,
        uint8Array.byteLength
      )
    }

    tagValue = (tagValue << 7) | (nextByte & 0b0111_1111)
    lengthOfTag++
  } while (nextByte & 0b1000_0000)

  return [tagValue, tagClassMarker, lengthOfTag] as const
}

export const serializeTag = (
  value: number,
  tagClass: TagMarker,
  uint8Array: Uint8Array,
  offset: number
) => {
  if (!isSafeUnsignedInteger(value)) {
    return new SerializeError(
      "unable to serialize tag - value is out of bounds"
    )
  }

  if (value < 0b0011_1111) {
    uint8Array[offset] = value | (tagClass << 6)
    return
  }

  uint8Array[offset] = (tagClass << 6) | 0b0011_1111

  let length = 0
  do {
    length++
  } while (value >= 1 << (7 * length))

  for (let index = length; index > 0; index--) {
    uint8Array[offset + index] =
      (value & 0b0111_1111) | (index === length ? 0 : 0b1000_0000)
    value = value >>> 7
  }
  return
}

export const predictTagLength = (value: number) => {
  if (!isSafeUnsignedInteger(value)) {
    return new SerializeError(
      "unable to serialize tag - value is out of bounds"
    )
  }

  if (value < 0b0011_1111) {
    return 1
  }

  let length = 0
  do {
    length++
  } while (value >= 1 << (7 * length))

  return length + 1
}
