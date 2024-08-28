import {
  base64ToUint8Array,
  stringToUint8Array,
  uint8ArrayToBase64,
  uint8ArrayToHex,
} from "uint8array-extras"
import { describe, test } from "vitest"

import {
  generateRandomCondition,
  generateTokenNonce,
  getPskEnvironment,
} from "../crypto/functions"
import { createMockCryptoContext } from "./mocks/crypto-context"

describe("Crypto", () => {
  describe("generateTokenNonce", () => {
    test("should generate a token nonce", ({ expect }) => {
      const context = createMockCryptoContext()
      const nonce = generateTokenNonce(context)

      expect(nonce.length).toBe(18)
      expect(uint8ArrayToHex(nonce)).toMatchInlineSnapshot(
        `"fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efee"`,
      )
    })
  })

  describe("generateRandomCondition", () => {
    test("should generate a random 32-byte condition", ({ expect }) => {
      const context = createMockCryptoContext()
      const condition = generateRandomCondition(context)

      expect(condition.length).toBe(32)
      expect(uint8ArrayToHex(condition)).toMatchInlineSnapshot(
        `"fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0"`,
      )
    })
  })

  describe("hash", () => {
    test("generates the expected condition", async ({ expect }) => {
      const context = createMockCryptoContext()
      const fulfillment = new Uint8Array(32)
      const wantCondition = base64ToUint8Array(
        "Zmh6rfhivXdsj8GLjp+OIAiXFIVu4jOzkCpZHQ1fKSU=",
      )
      const gotCondition = await context.hash(fulfillment)

      expect(gotCondition).toEqual(wantCondition)
    })
  })

  describe("getPskEnvironment", () => {
    function getTestEnvironment() {
      const context = createMockCryptoContext()
      const secret = stringToUint8Array("foo")
      return getPskEnvironment(context, secret)
    }

    describe("getEncryptionKey", () => {
      test("should generate the correct psk encryption key", async ({
        expect,
      }) => {
        const testEnvironment = getTestEnvironment()
        const key = await testEnvironment.getEncryptionKey()

        expect(key.length).toBe(32)
        expect(uint8ArrayToHex(key)).toMatchInlineSnapshot(
          `"cdf386314fee63e10e50213ab4ddd6844efb6c5fcdea94b4c123a63201003040"`,
        )
      })
    })

    describe("getFulfillmentKey", () => {
      test("should generate the correct key", async ({ expect }) => {
        const testEnvironment = getTestEnvironment()
        const key = await testEnvironment.getFulfillmentKey()

        expect(key.length).toBe(32)
        expect(uint8ArrayToHex(key)).toMatchInlineSnapshot(
          `"961f27189089a2cbdf51e3da534bb1c572b78cdbd5d98fb98afb75187d4cba1b"`,
        )
      })
    })

    describe("getFulfillment", () => {
      test("should correctly sign a message", async ({ expect }) => {
        const testEnvironment = getTestEnvironment()

        const message = stringToUint8Array("bar")
        const signature = await testEnvironment.getFulfillment(message)

        expect(signature.length).toBe(32)
        expect(uint8ArrayToHex(signature)).toMatchInlineSnapshot(
          `"c1803ed8ee0868f07da893d9184fd9973570d5ea7185b64483d3f886483bb92c"`,
        )
      })
    })

    describe("encrypt/decrypt", () => {
      test("encrypts data", async ({ expect }) => {
        const testEnvironment = getTestEnvironment()
        const cleartext = stringToUint8Array("foo bar")

        const ciphertext = await testEnvironment.encrypt(cleartext)

        expect(uint8ArrayToBase64(ciphertext)).toMatchInlineSnapshot(
          `"//79/Pv6+fj39vX0XVHEJAdqdFP2GxaHn8tDUqEw4Xeomsk="`,
        )
      })

      test("decrypts data", async ({ expect }) => {
        const testEnvironment = getTestEnvironment()
        const ciphertext = base64ToUint8Array(
          "//79/Pv6+fj39vX0XVHEJAdqdFP2GxaHn8tDUqEw4Xeomsk=",
        )

        const cleartext = await testEnvironment.decrypt(ciphertext)

        expect(cleartext).toEqual(stringToUint8Array("foo bar"))
      })

      test("decrypts encrypted data", async ({ expect }) => {
        const testEnvironment = getTestEnvironment()
        const cleartext = stringToUint8Array("foo bar")

        const ciphertext = await testEnvironment.encrypt(cleartext)
        const gotCleartext = await testEnvironment.decrypt(ciphertext)

        expect(gotCleartext).toEqual(cleartext)
      })
    })
  })
})
