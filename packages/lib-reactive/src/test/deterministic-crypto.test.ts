import { uint8ArrayToHex } from "uint8array-extras"
import { describe, test } from "vitest"

import { createMockDeterministicCrypto } from "../mocks/deterministic-crypto"
import type { Crypto } from "../types/base-modules/crypto"

describe("Deterministic Crypto", () => {
  test("should generate (not so) random bytes", ({ expect }) => {
    const crypto = createMockDeterministicCrypto({} as Crypto)
    const randomNumber = crypto.getRandomBytes(32)
    expect(uint8ArrayToHex(randomNumber)).toMatchInlineSnapshot(
      `"1f0796160c164e1fdcc306caed8497f7c424a4e4d66f5653f50d1f35e47638d9"`,
    )
  })

  test("should generate (not so) random bytes of a length that is not a multiple of 4", ({
    expect,
  }) => {
    const crypto = createMockDeterministicCrypto({} as Crypto)
    const randomNumber = crypto.getRandomBytes(31)
    expect(uint8ArrayToHex(randomNumber)).toMatchInlineSnapshot(
      `"1f0796160c164e1fdcc306caed8497f7c424a4e4d66f5653f50d1f35e47638"`,
    )
  })
})
