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
  return Boolean(crypto.getRandomBytes(1)[0]! & 0x01)
}

/**
 * Generates a random number between 0 (inclusive) and 1 (exclusive).
 *
 * This mimics the behavior of `Math.random()` but uses the provided entropy.
 *
 * @see https://github.com/nodejs/node/blob/f226350fcbebd4449fb0034fdaffa147e4de28ea/deps/v8/src/base/utils/random-number-generator.h#L111-L116
 */
export function randomNumber(crypto: Crypto) {
  const randomBytes = crypto.getRandomBytes(8)

  // Set first 12 bits to 0x3FF (positive, zero offset)
  //
  // Combined with a random mantissa this will produce a double precision float
  // in the range [1, 2)
  randomBytes[0] = 0b0011_1111
  randomBytes[1]! |= 0b1111_0000

  // Subtract 1 to transpose the range to [0, 1)
  const randomNumber =
    new DataView(
      randomBytes.buffer,
      randomBytes.byteOffset,
      randomBytes.byteLength,
    ).getFloat64(0) - 1

  return randomNumber
}
