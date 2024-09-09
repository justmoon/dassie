import {
  createCipheriv,
  createDecipheriv,
  createHash,
  createHmac,
  randomBytes,
} from "node:crypto"

import {
  type Crypto,
  DECRYPTION_FAILURE,
  type EncryptionAlgorithm,
  type HashAlgorithm,
  type MacAlgorithm,
} from "@dassie/lib-reactive"
import { bufferToUint8Array, isError } from "@dassie/lib-type-utils"

const MAC_ALGORITHM_MAP: Record<MacAlgorithm, string> = {
  "hmac-sha256": "sha256",
  "hmac-sha512": "sha512",
}

const ENCRYPTION_ALGORITHM_MAP: Record<
  EncryptionAlgorithm,
  {
    ivLength: number
    tagLength: number
  }
> = {
  "aes-128-gcm": {
    ivLength: 12,
    tagLength: 16,
  },
  "aes-256-gcm": {
    ivLength: 12,
    tagLength: 16,
  },
}

class BrowserCryptoImplementation implements Crypto {
  getRandomBytes(length: number): Uint8Array {
    return bufferToUint8Array(randomBytes(length))
  }

  hash(algorithm: HashAlgorithm, message: Uint8Array) {
    const hash = createHash(algorithm).update(message).digest()
    return bufferToUint8Array(hash)
  }

  createMac(algorithm: MacAlgorithm, key: Uint8Array) {
    return (message: Uint8Array) => {
      const hmac = createHmac(MAC_ALGORITHM_MAP[algorithm], key)
      hmac.update(message)
      return bufferToUint8Array(hmac.digest())
    }
  }

  createCryptor(algorithm: EncryptionAlgorithm, key: Uint8Array) {
    const algorithmInfo = ENCRYPTION_ALGORITHM_MAP[algorithm]

    return {
      encrypt: (plaintext: Uint8Array) => {
        const iv = this.getRandomBytes(algorithmInfo.ivLength)
        const cipher = createCipheriv(algorithm, key, iv)

        const ciphertext = Buffer.concat([
          cipher.update(plaintext),
          cipher.final(),
        ])

        return {
          iv,
          tag: bufferToUint8Array(cipher.getAuthTag()),
          ciphertext: bufferToUint8Array(ciphertext),
        }
      },

      decrypt({
        iv,
        tag,
        ciphertext,
      }: {
        iv: Uint8Array
        tag: Uint8Array
        ciphertext: Uint8Array
      }) {
        try {
          if (iv.length !== algorithmInfo.ivLength) {
            return DECRYPTION_FAILURE
          }

          if (tag.length !== algorithmInfo.tagLength) {
            return DECRYPTION_FAILURE
          }

          const decipher = createDecipheriv(algorithm, key, iv)
          decipher.setAuthTag(tag)

          const plaintext = Buffer.concat([
            decipher.update(ciphertext),
            decipher.final(),
          ])

          return bufferToUint8Array(plaintext)
        } catch (error: unknown) {
          if (isError(error) && error.name === "OperationError") {
            return DECRYPTION_FAILURE
          }

          throw error
        }
      },
    }
  }
}

export function createCrypto(): Crypto {
  return new BrowserCryptoImplementation()
}
