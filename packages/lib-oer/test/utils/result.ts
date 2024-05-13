import { hexToUint8Array } from "../../src/utils/hex"

export const serializedOk = (serializedData: string | Uint8Array) =>
  typeof serializedData === "string" ?
    hexToUint8Array(serializedData)
  : serializedData

export const parsedOk = <T>(length: number, value: T) => ({
  success: true,
  value,
  length,
})
