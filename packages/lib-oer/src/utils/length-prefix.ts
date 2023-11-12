import { ParseFailure, SerializeFailure } from "./failures"
import { type ParseContext, byteLength, isSafeUnsignedInteger } from "./parse"

export const parseLengthPrefix = (
  { uint8Array, allowNoncanonical }: ParseContext,
  offset: number,
  isEnumerationMode?: boolean,
): readonly [length: number, lengthOfLength: number] | ParseFailure => {
  const lengthByte = uint8Array[offset]

  if (lengthByte === undefined) {
    return new ParseFailure(
      "unable to read length prefix - end of buffer",
      uint8Array,
      uint8Array.byteLength,
    )
  }

  if (lengthByte & 0x80) {
    const lengthOfLength = lengthByte & 0x7f

    if (offset + lengthOfLength + 1 > uint8Array.length) {
      return new ParseFailure(
        "unable to read length prefix - end of buffer",
        uint8Array,
        uint8Array.byteLength,
      )
    }

    if (lengthOfLength > 6) {
      return new ParseFailure(
        "unable to read length prefix - length too long",
        uint8Array,
        offset,
      )
    }

    let length = 0
    for (let index = 0; index < lengthOfLength; index++) {
      // We have checked the length, so we can assert that this is not undefined
      length = ((length << 8) | uint8Array[offset + index + 1]!) >>> 0
    }

    if (length + offset + lengthOfLength + 1 > uint8Array.length) {
      return new ParseFailure(
        "unable to read value after length prefix - end of buffer",
        uint8Array,
        uint8Array.byteLength,
      )
    }

    // Check canonicality
    if (!allowNoncanonical) {
      if (!isEnumerationMode && length <= 0x7f) {
        return new ParseFailure(
          "non-canonical encoding - length prefix is not minimal (length <= 0x7f but not encoded as a single byte)",
          uint8Array,
          offset,
        )
      }

      if (length < 2 ** ((lengthOfLength - 1) * 8) - 1) {
        return new ParseFailure(
          "non-canonical encoding - length prefix is not minimal (could be encoded in fewer bytes)",
          uint8Array,
          offset,
        )
      }
    }

    return [length, lengthOfLength + 1] as const
  }

  const length = lengthByte

  if (offset + length + 1 > uint8Array.length) {
    return new ParseFailure(
      "unable to read length prefix - end of buffer",
      uint8Array,
      uint8Array.length,
    )
  }

  return [length, 1] as const
}

export const serializeLengthPrefix = (
  length: number,
  uint8Array: Uint8Array,
  offset: number,
) => {
  if (length > 0x7f) {
    if (!isSafeUnsignedInteger(length)) {
      return new SerializeFailure(
        "unable to serialize length prefix - length is too large",
      )
    }
    const lengthOfLength = byteLength(length)

    // TODO: Could implement support for serializing variable octet strings larger than 4 GB
    if (lengthOfLength > 4) {
      return new SerializeFailure(
        "unable to serialize length prefix - length is too large",
      )
    }

    uint8Array[offset] = 0x80 | lengthOfLength

    for (let index = 0; index < lengthOfLength; index++) {
      uint8Array[offset + index + 1] =
        (length >>> (8 * (lengthOfLength - 1 - index))) & 0xff
    }

    return 1 + lengthOfLength
  } else {
    uint8Array[offset] = length
    return 1
  }
}

export const predictLengthPrefixLength = (length: number) => {
  if (!isSafeUnsignedInteger(length)) {
    return new SerializeFailure(
      "unable to serialize variable length field - value is out of bounds",
    )
  }
  return 1 + (length > 127 ? byteLength(length) : 0)
}
