import { concatUint8Arrays, stringToUint8Array } from "uint8array-extras"

import type { AesCryptor, Crypto, HmacSigner } from "@dassie/lib-reactive"

import type { DecryptionFailure } from "./failures/decryption-failure"

const HMAC_ALGORITHM = "hmac-sha256"
const AES_ALGORITHM = "aes-256-gcm"

export const TOKEN_NONCE_LENGTH = 18

export function generateTokenNonce(context: Crypto): Uint8Array {
  return context.getRandomBytes(TOKEN_NONCE_LENGTH)
}

export const CONDITION_LENGTH = 32

export function generateRandomCondition(context: Crypto): Uint8Array {
  return context.getRandomBytes(CONDITION_LENGTH)
}

const ENCRYPTION_KEY_STRING = stringToUint8Array("ilp_stream_encryption")

const FULFILLMENT_GENERATION_STRING = stringToUint8Array(
  "ilp_stream_fulfillment",
)

export function getPskEnvironment(context: Crypto, secret: Uint8Array) {
  const hmacSigner = context.createMac(HMAC_ALGORITHM, secret)
  let pskEncryptionKey: Promise<Uint8Array> | undefined
  let fulfillmentKey: Promise<Uint8Array> | undefined
  let fulfillmentGenerator: Promise<HmacSigner> | undefined
  let cryptorCache: Promise<AesCryptor> | undefined

  return {
    getEncryptionKey(): Promise<Uint8Array> {
      if (!pskEncryptionKey) {
        pskEncryptionKey = Promise.resolve(hmacSigner(ENCRYPTION_KEY_STRING))
      }

      return pskEncryptionKey
    },

    getFulfillmentKey(): Promise<Uint8Array> {
      if (!fulfillmentKey) {
        fulfillmentKey = Promise.resolve(
          hmacSigner(FULFILLMENT_GENERATION_STRING),
        )
      }

      return fulfillmentKey
    },

    async getFulfillment(message: Uint8Array): Promise<Uint8Array> {
      if (!fulfillmentGenerator) {
        fulfillmentGenerator = this.getFulfillmentKey().then((key) =>
          context.createMac(HMAC_ALGORITHM, key),
        )
      }

      return (await fulfillmentGenerator)(message)
    },

    async encrypt(plaintext: Uint8Array): Promise<Uint8Array> {
      if (!cryptorCache) {
        cryptorCache = this.getEncryptionKey().then((key) =>
          context.createCryptor(AES_ALGORITHM, key),
        )
      }

      const cryptor = await cryptorCache

      const { iv, tag, ciphertext } = await cryptor.encrypt(plaintext)

      return concatUint8Arrays([iv, tag, ciphertext])
    },

    async decrypt(
      ciphertext: Uint8Array,
    ): Promise<Uint8Array | DecryptionFailure> {
      if (!cryptorCache) {
        cryptorCache = this.getEncryptionKey().then((key) =>
          context.createCryptor(AES_ALGORITHM, key),
        )
      }

      const cryptor = await cryptorCache

      const iv = ciphertext.subarray(0, 12)
      const tag = ciphertext.subarray(12, 28)
      const ciphertextWithoutIvAndTag = ciphertext.subarray(28)

      return cryptor.decrypt({ iv, tag, ciphertext: ciphertextWithoutIvAndTag })
    },
  }
}

export type PskEnvironment = ReturnType<typeof getPskEnvironment>
