import { describe, test } from "vitest"

import { bufferToUint8Array } from "@dassie/lib-type-utils"

import { createClient } from "../connection/client"
import { createCompatibilityServer } from "./mocks/compatibility"
import { createMockCryptoContext } from "./mocks/crypto-context"

describe("Client Compatibility", () => {
  test("should be able to measure 1:1 exchange rate", async ({ expect }) => {
    const server = await createCompatibilityServer()

    const { destinationAccount, sharedSecret } =
      server.server.generateAddressAndSecret()

    const client = createClient({
      context: {
        crypto: createMockCryptoContext(),
        logger: console,
        sendPacket: server.sendPacket,
        getDateNow() {
          return new Date("2024-08-29T10:44:29.380Z").valueOf()
        },
      },
      destination: destinationAccount,
      secret: bufferToUint8Array(sharedSecret),
    })

    const exchangeRate = await client.measureExchangeRate()

    expect(exchangeRate).toMatchInlineSnapshot(`
      [
        1n,
        1n,
      ]
    `)
  })
})
