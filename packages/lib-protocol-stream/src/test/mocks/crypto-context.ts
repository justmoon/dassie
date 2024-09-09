import type { Crypto } from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

export function createMockCryptoContext(): Crypto {
  let nonce = 0

  return Object.assign(createCrypto(), {
    getRandomBytes: (length: number) => {
      const result = new Uint8Array(length)

      for (let index = 0; index < length; index++) {
        result[index] = 255 - (nonce % 256)
        nonce++
      }

      return result
    },
  })
}
