import { murmurHash3_128_X86 } from "../internal/algorithms/murmurhash3-128-x86"
import {
  DEFAULT_SEED,
  Xoshiro128PlusPlus,
} from "../internal/algorithms/xoshiro128pp"
import type { Crypto } from "../types/base-modules/crypto"

export function createMockDeterministicCrypto(
  crypto: Crypto,
  seed: Uint32Array = DEFAULT_SEED,
): Crypto {
  const randomGenerator = new Xoshiro128PlusPlus(
    typeof seed === "string" ? generateSeedFromString(seed) : seed,
  )

  return Object.assign(crypto, {
    getRandomBytes: (length: number) => {
      const data = new Uint32Array(Math.ceil(length / 4))

      for (let index = 0; index < data.length; index++) {
        data[index] = randomGenerator.next()
      }

      return new Uint8Array(data.buffer, 0, length)
    },

    generateRsaKeyPair: () => {
      throw new Error(
        "Deterministic RSA key generation is not implemented in this mock",
      )
    },
  })
}

export function generateSeedFromString(seed: string): Uint32Array {
  return murmurHash3_128_X86(seed)
}
