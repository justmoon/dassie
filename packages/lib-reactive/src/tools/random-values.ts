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

export function* sampleBoolean(crypto: Crypto): Generator<boolean> {
  for (;;) {
    const randomByte = crypto.getRandomBytes(1)[0]!
    yield Boolean(randomByte & 0x01)
    yield Boolean(randomByte & 0x02)
    yield Boolean(randomByte & 0x04)
    yield Boolean(randomByte & 0x08)
    yield Boolean(randomByte & 0x10)
    yield Boolean(randomByte & 0x20)
    yield Boolean(randomByte & 0x40)
    yield Boolean(randomByte & 0x80)
  }
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

/**
 * Creates a generator which produces random numbers from a Gaussian distribution.
 *
 * Implementation of the Box-Muller transform.
 *
 * @param crypto - Cryptographic module to use as an entropy source.
 * @param mean - Mean of the Gaussian distribution.
 * @param standardDeviation - Standard deviation of the Gaussian distribution.
 */
export function* sampleGaussianDistribution(
  crypto: Crypto,
  mean = 0,
  standardDeviation = 1,
): Generator<number, never> {
  for (;;) {
    // Get two uniform random numbers in the range (0, 1)
    let uniformRandom1 = 0
    let uniformRandom2 = 0
    while (uniformRandom1 === 0) uniformRandom1 = randomNumber(crypto)
    while (uniformRandom2 === 0) uniformRandom2 = randomNumber(crypto)

    const angle = 2 * Math.PI * uniformRandom1
    const magnitude = Math.sqrt(-2 * Math.log(uniformRandom2))

    yield mean + magnitude * Math.cos(angle) * standardDeviation
    yield mean + magnitude * Math.sin(angle) * standardDeviation
  }
}

/**
 *
 *
 * @param crypto - Cryptographic module to use as an entropy source.
 * @param mean - Mean of the underlying normal distribution.
 * @param standardDeviation - Standard deviation of the underlying normal distribution.
 */
export function* sampleLogNormalDistribution(
  crypto: Crypto,
  mean = 0,
  standardDeviation = 1,
): Generator<number, never> {
  for (const sample of sampleGaussianDistribution(
    crypto,
    mean,
    standardDeviation,
  )) {
    yield Math.exp(sample)
  }

  // Workaround for https://github.com/microsoft/TypeScript/issues/56363
  throw new Error("unreachable")
}
