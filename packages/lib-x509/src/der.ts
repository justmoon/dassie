import { hexToUint8Array } from "uint8array-extras"

export function encodeField(tag: number, value: Uint8Array) {
  if (tag > 0xff) {
    throw new Error("Tag must be a single byte")
  }

  const length = value.length
  if (length < 128) {
    return new Uint8Array([tag, length, ...value])
  } else {
    const lengthBytes = encodeLength(length)
    return new Uint8Array([tag, ...lengthBytes, ...value])
  }
}

export function encodeLength(length: number) {
  if (length < 128) {
    return new Uint8Array([length])
  } else if (length < 256) {
    return new Uint8Array([0x81, length])
  } else if (length < 65_536) {
    return new Uint8Array([0x82, length >> 8, length & 0xff])
  } else {
    throw new Error("Fields > 65535 are not supported in this implementation")
  }
}

export function bigintToUint8Array(value: bigint) {
  const hex = value.toString(16)
  return hexToUint8Array(hex.length % 2 === 0 ? hex : "0" + hex)
}
