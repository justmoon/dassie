import { uint8ArrayToHex } from "uint8array-extras"

import type { Crypto } from "../types/base-modules/crypto"

export function randomBigInt(crypto: Crypto, min: bigint, max: bigint): bigint {
  if (min >= max) {
    throw new Error("min must be less than max")
  }

  const range = max - min
  const rangeBits = range.toString(2).length
  const rangeBytes = Math.ceil(rangeBits / 8)
  const highByteMask = 0xff << (rangeBytes * 8 - rangeBits)

  let randomValue: bigint
  do {
    const randomBuffer = crypto.getRandomBytes(rangeBytes)
    randomBuffer[0]! &= highByteMask
    randomValue = BigInt("0x" + uint8ArrayToHex(randomBuffer))
  } while (randomValue > range)

  return randomValue + min
}

export function randomBoolean(crypto: Crypto): boolean {
  return crypto.getRandomBytes(1)[0]! % 2 === 0
}

/**
 * Generates a random number exactly like `Math.random()`.
 */
export function randomNumber(crypto: Crypto) {
  const randomBytes = crypto.getRandomBytes(8)

  // We need to convert the bytes to a number in the range [0, 1]
  // because Math.random() returns a number in the range [0, 1).
  const randomNumber = Number(
    new DataView(randomBytes.buffer, randomBytes.byteOffset).getFloat64(0),
  )

  return randomNumber
}
