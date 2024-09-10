import { uint8ArrayToString } from "uint8array-extras"

import {
  type Crypto,
  DECRYPTION_FAILURE,
  type EncryptionAlgorithm,
  type HashAlgorithm,
  type MacAlgorithm,
  type RsaKeyPair,
} from "@dassie/lib-reactive"
import { isError } from "@dassie/lib-type-utils"

const HASH_ALGORITHM_MAP: Record<HashAlgorithm, string> = {
  sha256: "SHA-256",
  sha512: "SHA-512",
}

const MAC_ALGORITHM_MAP: Record<
  MacAlgorithm,
  AlgorithmIdentifier & HmacImportParams
> = {
  "hmac-sha256": {
    name: "HMAC",
    hash: { name: "SHA-256" },
  },
  "hmac-sha512": {
    name: "HMAC",
    hash: { name: "SHA-512" },
  },
}

const ENCRYPTION_ALGORITHM_MAP: Record<
  EncryptionAlgorithm,
  {
    name: string
    ivLength: number
    tagLength: number
  }
> = {
  "aes-128-gcm": {
    name: "AES-GCM",
    ivLength: 12,
    tagLength: 16,
  },
  "aes-256-gcm": {
    name: "AES-GCM",
    ivLength: 12,
    tagLength: 16,
  },
}

class BrowserCryptoImplementation implements Crypto {
  getRandomBytes(length: number): Uint8Array {
    return crypto.getRandomValues(new Uint8Array(length))
  }

  async hash(algorithm: HashAlgorithm, message: Uint8Array) {
    const hash = await crypto.subtle.digest(
      HASH_ALGORITHM_MAP[algorithm],
      message,
    )
    return new Uint8Array(hash)
  }

  createMac(algorithm: MacAlgorithm, key: Uint8Array) {
    const keyObjectPromise = crypto.subtle.importKey(
      "raw",
      key,
      MAC_ALGORITHM_MAP[algorithm],
      false,
      ["sign", "verify"],
    )

    return async (message: Uint8Array) => {
      const keyObject = await keyObjectPromise
      const signature = await crypto.subtle.sign(
        MAC_ALGORITHM_MAP[algorithm],
        keyObject,
        message,
      )
      return new Uint8Array(signature)
    }
  }

  createCryptor(algorithm: EncryptionAlgorithm, key: Uint8Array) {
    const algorithmInfo = ENCRYPTION_ALGORITHM_MAP[algorithm]

    const keyObjectPromise = crypto.subtle.importKey(
      "raw",
      key,
      algorithmInfo.name,
      false,
      ["encrypt", "decrypt"],
    )

    return {
      encrypt: async (plaintext: Uint8Array) => {
        const keyObject = await keyObjectPromise

        const iv = this.getRandomBytes(algorithmInfo.ivLength)
        const ciphertext = new Uint8Array(
          await crypto.subtle.encrypt(
            {
              name: algorithmInfo.name,
              iv,
              tagLength: algorithmInfo.tagLength * 8,
            },
            keyObject,
            plaintext,
          ),
        )

        return {
          iv,
          tag: ciphertext.subarray(
            ciphertext.byteLength - algorithmInfo.tagLength,
          ),
          ciphertext: ciphertext.subarray(
            0,
            ciphertext.byteLength - algorithmInfo.tagLength,
          ),
        }
      },

      async decrypt({
        iv,
        tag,
        ciphertext,
      }: {
        iv: Uint8Array
        tag: Uint8Array
        ciphertext: Uint8Array
      }) {
        try {
          const keyObject = await keyObjectPromise

          // Extract the IV and move the tag to the end of the ciphertext
          const combined = new Uint8Array(
            ciphertext.byteLength + algorithmInfo.tagLength,
          )
          combined.set(ciphertext)
          combined.set(tag, ciphertext.byteLength)

          const plaintext = await crypto.subtle.decrypt(
            {
              name: algorithmInfo.name,
              iv,
            },
            keyObject,
            ciphertext,
          )

          return new Uint8Array(plaintext)
        } catch (error: unknown) {
          if (isError(error) && error.name === "OperationError") {
            return DECRYPTION_FAILURE
          }

          throw error
        }
      },
    }
  }

  async generateRsaKeyPair(modulusLength: number): Promise<RsaKeyPair> {
    const keyPair = await crypto.subtle.generateKey(
      {
        name: "RSA-OAEP",
        modulusLength,
        publicExponent: new Uint8Array([1, 0, 1]),
        hash: "SHA-256",
      },
      true,
      ["encrypt", "decrypt"],
    )

    return createRsaKeyPair(keyPair)
  }

  async importRsaKeyPair(
    privateKeyData: string | Uint8Array,
  ): Promise<RsaKeyPair> {
    if (typeof privateKeyData === "string") {
      throw new TypeError("PEM format is not supported in this implementation")
    }

    const privateKey = await crypto.subtle.importKey(
      "pkcs8",
      privateKeyData,
      {
        name: "RSA-OAEP",
        hash: "SHA-256",
      },
      true,
      ["sign", "verify"],
    )

    const publicKey = await derivePublicKeyFromPrivateKey(privateKey)

    return createRsaKeyPair({ privateKey, publicKey })
  }
}

export function createCrypto(): Crypto {
  return new BrowserCryptoImplementation()
}

function createRsaKeyPair(keys: CryptoKeyPair): RsaKeyPair {
  return {
    async getPublicKeyPem() {
      return uint8ArrayToString(
        new Uint8Array(await crypto.subtle.exportKey("spki", keys.publicKey)),
      )
    },
    async getPrivateKeyPem() {
      return uint8ArrayToString(
        new Uint8Array(await crypto.subtle.exportKey("pkcs8", keys.privateKey)),
      )
    },

    async sign(message: Uint8Array) {
      const signature = await crypto.subtle.sign(
        "RSASSA-PKCS1-v1_5",
        keys.privateKey,
        message,
      )
      return new Uint8Array(signature)
    },

    async verify(message: Uint8Array, signature: Uint8Array) {
      return crypto.subtle.verify(
        "RSASSA-PKCS1-v1_5",
        keys.publicKey,
        signature,
        message,
      )
    },
  }
}

async function derivePublicKeyFromPrivateKey(
  privateKey: CryptoKey,
): Promise<CryptoKey> {
  const jwk = await crypto.subtle.exportKey("jwk", privateKey)

  // remove private data from JWK
  delete jwk.d
  delete jwk.dp
  delete jwk.dq
  delete jwk.q
  delete jwk.qi
  jwk.key_ops = ["encrypt", "wrapKey"]

  // import public key
  const publicKey = await crypto.subtle.importKey(
    "jwk",
    jwk,
    { name: "RSA-OAEP", hash: "SHA-512" },
    true,
    ["encrypt", "wrapKey"],
  )
  return publicKey
}
