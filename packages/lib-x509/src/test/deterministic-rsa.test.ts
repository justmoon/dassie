import { describe, test } from "vitest"

import { createMockDeterministicCrypto } from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

import {
  generateCandidatePrime,
  generateRsaPrivateKey,
} from "./mocks/deterministic-rsa"

describe("Deterministic RSA", () => {
  test("should generate a probably prime", ({ expect }) => {
    const crypto = createMockDeterministicCrypto(createCrypto())

    const prime = generateCandidatePrime(crypto, 256, 10)

    expect(prime).toBeGreaterThan(2n ** (256n - 1n))
    expect(prime).toBeLessThan(2n ** 256n)
    expect(prime).toMatchInlineSnapshot(
      `95376559146912120247391526836318564295006466684117348169866715772212294863781n`,
    )
  })
  test("should generate all the numbers that constitute an RSA private key", ({
    expect,
  }) => {
    const crypto = createMockDeterministicCrypto(createCrypto())

    const privateKey = generateRsaPrivateKey(crypto, 256)

    expect(privateKey).toMatchInlineSnapshot(`
      {
        "coefficient": 45457243918652619074321965289328805273656629280515885840336619194001385098097n,
        "exponent1": 34665450644360387327660194535012408280926103367802542585199584504846069604273n,
        "exponent2": 73944686680928480844779120197475364791676013045677398046841499638933028738627n,
        "modulus": 7979655345836733361536377749638643733614047795639403094961007949505419923839286439959163500679006815908069272245008314881901938432175281494497828387291987n,
        "primeP": 95376559146912120247391526836318564295006466684117348169866715772212294863781n,
        "primeQ": 83664743383595632980410013300104327855119224953378789700047500333835504107927n,
        "privateExponent": 2995369045927178195937507499868170262456920362856477341641145560000958779717806446958997638875247410355455941329778053805784450690178164096856984516429913n,
        "publicExponent": 65537n,
      }
    `)
  })
})
