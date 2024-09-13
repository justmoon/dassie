import { describe, test } from "vitest"

import { unwrapFailure } from "@dassie/lib-type-utils"

import { createClient } from "../connection/client"
import { createServer } from "../server/create"
import { createTestEnvironment } from "./mocks/test-environment"

describe("Connection", () => {
  test("should be able to measure 1:1 exchange rate", async ({ expect }) => {
    const environment = createTestEnvironment()
    const server = unwrapFailure(
      await createServer({
        context: environment.createContext({ name: "server" }),
      }),
    )

    const credentials = server.generateCredentials()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({ name: "client" }),
        credentials,
      }),
    )

    const exchangeRate = await client.measureExchangeRate()

    expect(exchangeRate).toMatchInlineSnapshot(`
      [
        1n,
        1n,
      ]
    `)

    await environment.dispose()
  })

  test("should be able to measure 2:3 exchange rate", async ({ expect }) => {
    const environment = createTestEnvironment()
    const server = unwrapFailure(
      await createServer({
        context: environment.createContext({
          name: "server",
          unitsPerToken: 2_000_000n,
        }),
      }),
    )

    const credentials = server.generateCredentials()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({
          name: "client",
          unitsPerToken: 3_000_000n,
        }),
        credentials,
      }),
    )

    const exchangeRate = await client.measureExchangeRate()

    expect(exchangeRate).toMatchInlineSnapshot(`
      [
        3n,
        2n,
      ]
    `)

    await environment.dispose()
  })

  test("should be able to measure high-precision exchange rate", async ({
    expect,
  }) => {
    const environment = createTestEnvironment()
    const server = unwrapFailure(
      await createServer({
        context: environment.createContext({
          name: "server",
          unitsPerToken: BigInt(Math.floor(1e6 * Math.PI)),
        }),
      }),
    )

    const credentials = server.generateCredentials()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({
          name: "client",
          unitsPerToken: BigInt(Math.floor(1e6 * Math.E)),
        }),
        credentials,
      }),
    )

    const exchangeRate = await client.measureExchangeRate()

    expect(exchangeRate).toMatchInlineSnapshot(`
      [
        108156986967n,
        125000000000n,
      ]
    `)

    await environment.dispose()
  })

  test("should be able to measure exchange rate when packet size is limited", async ({
    expect,
  }) => {
    const environment = createTestEnvironment({
      maxPacketAmount: BigInt(1e10),
    })
    const server = unwrapFailure(
      await createServer({
        context: environment.createContext({
          name: "server",
          unitsPerToken: BigInt(Math.floor(1e6 * Math.PI)),
        }),
      }),
    )

    const credentials = server.generateCredentials()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({
          name: "client",
          unitsPerToken: BigInt(Math.floor(1e6 * Math.E)),
        }),
        credentials,
      }),
    )

    const exchangeRate = await client.measureExchangeRate()

    expect(exchangeRate).toMatchInlineSnapshot(`
      [
        173051179n,
        200000000n,
      ]
    `)

    await environment.dispose()
  })

  test("should be able to send money", async ({ expect }) => {
    const environment = createTestEnvironment()
    const server = unwrapFailure(
      await createServer({
        context: environment.createContext({ name: "server" }),
      }),
    )

    const credentials = server.generateCredentials()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({ name: "client" }),
        credentials,
      }),
    )

    unwrapFailure(client.setExchangeRate([1n, 1n]))

    let moneyReceived = 0n
    server.on("connection", (connection) => {
      connection.on("stream", (stream) => {
        stream.addReceiveAmount(1000n)

        stream.on("money", (amount) => {
          moneyReceived += amount
        })
      })
    })

    const stream = client.createStream()

    unwrapFailure(await stream.send({ amount: 1000n }))

    expect(moneyReceived).toBe(1000n)

    await environment.dispose()
  })
})
