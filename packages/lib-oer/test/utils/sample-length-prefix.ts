import { hexToUint8Array, uint8ArrayToHex } from "../../src"

export const getLengthPrefixAsHex = (byteLength: number) => {
  if (byteLength <= 127) {
    return byteLength.toString(16).padStart(2, "0")
  } else if (byteLength <= 0xff) {
    return "81" + byteLength.toString(16).padStart(2, "0")
  } else if (byteLength <= 0xff_ff) {
    return "82" + byteLength.toString(16).padStart(4, "0")
  } else if (byteLength <= 0xff_ff_ff) {
    return "83" + byteLength.toString(16).padStart(6, "0")
  } else if (byteLength <= 0xff_ff_ff_ff) {
    return "84" + byteLength.toString(16).padStart(8, "0")
  }

  throw new Error(
    "Test data generator does not support byte lengths greater than 0xff_ff_ff_ff",
  )
}

export const addLengthPrefix = (buffer: Uint8Array) => {
  return hexToUint8Array(
    `${getLengthPrefixAsHex(buffer.byteLength)} ${uint8ArrayToHex(buffer)}`,
  )
}
