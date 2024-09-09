import type { Crypto } from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

const DEFAULT_SEED = new Uint32Array([
  0x13_67_bd_3b, 0x83_31_fa_de, 0x7b_96_f6_b5, 0xb4_9e_9f_58,
])

export function createMockCryptoContext(): Crypto {
  const randomGenerator = new Xoshiro128PlusPlus()

  return Object.assign(createCrypto(), {
    getRandomBytes: (length: number) => {
      const data = new Uint32Array(Math.ceil(length / 4))

      for (let index = 0; index < data.length; index++) {
        data[index] = randomGenerator.next()
      }

      const result = new Uint8Array(data.buffer)

      return result.subarray(0, length)
    },
  })
}

class Xoshiro128PlusPlus {
  private state: Uint32Array

  constructor(seed: Uint32Array = DEFAULT_SEED) {
    if (seed.length !== 4) {
      throw new Error("Seed must have 4 elements.")
    }
    this.state = new Uint32Array(seed)
  }

  // Rotate left function (similar to rotl)
  private rotl(x: number, k: number): number {
    return (x << k) | (x >>> (32 - k))
  }

  // Generate the next random number
  next(): number {
    const result =
      this.rotl(this.state[0]! + this.state[3]!, 7) + this.state[0]!

    const t = this.state[1]! << 9

    this.state[2]! ^= this.state[0]!
    this.state[3]! ^= this.state[1]!
    this.state[1]! ^= this.state[2]!
    this.state[0]! ^= this.state[3]!

    this.state[2]! ^= t

    this.state[3] = this.rotl(this.state[3]!, 11)

    return result >>> 0 // Ensure it returns a 32-bit unsigned number
  }
}
