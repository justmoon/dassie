import type { CryptoContext } from "../../crypto/context"
import * as webContext from "../../crypto/contexts/web-crypto"

export function createMockCryptoContext(): CryptoContext {
  let nonce = 0

  return {
    ...webContext,

    getRandomBytes: (length: number) => {
      const result = new Uint8Array(length)

      for (let index = 0; index < length; index++) {
        result[index] = 255 - (nonce % 256)
        nonce++
      }

      return result
    },
  }
}
