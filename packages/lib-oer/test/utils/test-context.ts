import { hexToUint8Array } from "../../src"
import type { ParseContext, SerializeContext } from "../../src/utils/parse"

export const createTestParseContext = (hex: string): ParseContext => {
  const uint8Array = hexToUint8Array(hex)

  return {
    uint8Array,
    dataView: new DataView(
      uint8Array.buffer,
      uint8Array.byteOffset,
      uint8Array.byteLength,
    ),
    allowNoncanonical: false,
  }
}

export const createTestSerializeContext = (hex: string): SerializeContext => {
  const uint8Array = new Uint8Array(hex.length / 2)

  return {
    uint8Array,
    dataView: new DataView(
      uint8Array.buffer,
      uint8Array.byteOffset,
      uint8Array.byteLength,
    ),
  }
}
