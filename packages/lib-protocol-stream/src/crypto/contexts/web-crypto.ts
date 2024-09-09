/* eslint-disable n/no-unsupported-features/node-builtins */
import { isError } from "@dassie/lib-type-utils"

import type { CryptoContext, Cryptor, HmacSigner } from "../context"
import { DECRYPTION_FAILURE } from "../failures/decryption-failure"

const HASH_ALGORITHM = "SHA-256"
const HMAC_ALGORITHM = {
  name: "HMAC",
  hash: { name: HASH_ALGORITHM },
}
const ENCRYPTION_ALGORITHM = "AES-GCM"
const IV_LENGTH = 12
const AUTH_TAG_BYTES = 16
const AUTH_TAG_BITS = AUTH_TAG_BYTES * 8

export function getRandomBytes(length: number): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(length))
}

export async function hash(message: Uint8Array): Promise<Uint8Array> {
  const hash = await crypto.subtle.digest(HASH_ALGORITHM, message)
  return new Uint8Array(hash)
}

export function createHmac(key: Uint8Array): HmacSigner {
  const keyObjectPromise = crypto.subtle.importKey(
    "raw",
    key,
    HMAC_ALGORITHM,
    false,
    ["sign", "verify"],
  )

  return async (message: Uint8Array) => {
    const keyObject = await keyObjectPromise
    const signature = await crypto.subtle.sign(
      HMAC_ALGORITHM,
      keyObject,
      message,
    )
    return new Uint8Array(signature)
  }
}

export function createAesCryptor(
  this: CryptoContext,
  key: Uint8Array,
): Cryptor {
  const keyObjectPromise = crypto.subtle.importKey(
    "raw",
    key,
    ENCRYPTION_ALGORITHM,
    false,
    ["encrypt", "decrypt"],
  )

  return {
    encrypt: async (plaintext: Uint8Array) => {
      const keyObject = await keyObjectPromise

      const iv = this.getRandomBytes(IV_LENGTH)
      const ciphertext = new Uint8Array(
        await crypto.subtle.encrypt(
          {
            name: ENCRYPTION_ALGORITHM,
            iv,
            tagLength: AUTH_TAG_BITS,
          },
          keyObject,
          plaintext,
        ),
      )

      // Prepend the IV and move the auth tag in front of the ciphertext
      const output = new Uint8Array(ciphertext.byteLength + IV_LENGTH)
      output.set(iv)
      output.set(
        // Auth tag
        ciphertext.subarray(ciphertext.byteLength - AUTH_TAG_BYTES),
        IV_LENGTH,
      )
      output.set(
        // Ciphertext
        ciphertext.subarray(0, ciphertext.byteLength - AUTH_TAG_BYTES),
        IV_LENGTH + AUTH_TAG_BYTES,
      )

      return output
    },

    async decrypt(ciphertext: Uint8Array) {
      try {
        const keyObject = await keyObjectPromise

        // Extract the IV and move the tag to the end of the ciphertext
        const iv = ciphertext.subarray(0, IV_LENGTH)
        const tag = ciphertext.subarray(IV_LENGTH, IV_LENGTH + AUTH_TAG_BYTES)
        const encryptedBlocks = ciphertext.subarray(IV_LENGTH + AUTH_TAG_BYTES)
        ciphertext = new Uint8Array(encryptedBlocks.byteLength + AUTH_TAG_BYTES)
        ciphertext.set(encryptedBlocks)
        ciphertext.set(tag, encryptedBlocks.byteLength)

        const plaintext = await crypto.subtle.decrypt(
          {
            name: ENCRYPTION_ALGORITHM,
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
