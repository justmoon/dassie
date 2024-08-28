import { stringToUint8Array } from "uint8array-extras"

import type { CryptoContext, Cryptor, HmacSigner } from "./context"

export const TOKEN_NONCE_LENGTH = 18

export function generateTokenNonce(context: CryptoContext): Uint8Array {
  return context.getRandomBytes(TOKEN_NONCE_LENGTH)
}

export const CONDITION_LENGTH = 32

export function generateRandomCondition(context: CryptoContext): Uint8Array {
  return context.getRandomBytes(CONDITION_LENGTH)
}

const ENCRYPTION_KEY_STRING = stringToUint8Array("ilp_stream_encryption")

const FULFILLMENT_GENERATION_STRING = stringToUint8Array(
  "ilp_stream_fulfillment",
)

export function getPskEnvironment(context: CryptoContext, secret: Uint8Array) {
  const hmacSigner = context.createHmac(secret)
  let pskEncryptionKey: Promise<Uint8Array> | undefined
  let fulfillmentKey: Promise<Uint8Array> | undefined
  let fulfillmentGenerator: Promise<HmacSigner> | undefined
  let cryptorCache: Promise<Cryptor> | undefined

  return {
    getEncryptionKey(): Promise<Uint8Array> {
      if (!pskEncryptionKey) {
        pskEncryptionKey = hmacSigner(ENCRYPTION_KEY_STRING)
      }

      return pskEncryptionKey
    },

    getFulfillmentKey(): Promise<Uint8Array> {
      if (!fulfillmentKey) {
        fulfillmentKey = hmacSigner(FULFILLMENT_GENERATION_STRING)
      }

      return fulfillmentKey
    },

    async getFulfillment(message: Uint8Array): Promise<Uint8Array> {
      if (!fulfillmentGenerator) {
        fulfillmentGenerator = this.getFulfillmentKey().then((key) =>
          context.createHmac(key),
        )
      }

      return (await fulfillmentGenerator)(message)
    },

    async encrypt(plaintext: Uint8Array): Promise<Uint8Array> {
      if (!cryptorCache) {
        cryptorCache = this.getEncryptionKey().then((key) =>
          context.createAesCryptor(key),
        )
      }

      const cryptor = await cryptorCache

      return cryptor.encrypt(plaintext)
    },

    async decrypt(ciphertext: Uint8Array): Promise<Uint8Array> {
      if (!cryptorCache) {
        cryptorCache = this.getEncryptionKey().then((key) =>
          context.createAesCryptor(key),
        )
      }

      const cryptor = await cryptorCache

      return cryptor.decrypt(ciphertext)
    },
  }
}
