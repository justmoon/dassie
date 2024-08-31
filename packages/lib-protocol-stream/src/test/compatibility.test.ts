import type { Connection, DataAndMoneyStream } from "ilp-protocol-stream"
import { describe, test } from "vitest"

import { setTimeout } from "node:timers/promises"

import { bufferToUint8Array, unwrapFailure } from "@dassie/lib-type-utils"

import { createClient } from "../connection/client"
import { createServer } from "../server/create"
import {
  createCompatibilityClient,
  createCompatibilityServer,
} from "./mocks/compatibility"
import { createTestEnvironment } from "./mocks/test-environment"

describe("Client Compatibility", () => {
  test("should be able to measure 1:1 exchange rate", async ({ expect }) => {
    const environment = createTestEnvironment()
    const server = await createCompatibilityServer(
      environment.createContext({ name: "server" }),
    )

    const { destinationAccount, sharedSecret } =
      server.generateAddressAndSecret()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({ name: "client" }),
        remoteAddress: destinationAccount,
        secret: bufferToUint8Array(sharedSecret),
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

  test("should be able to send money", async ({ expect }) => {
    const environment = createTestEnvironment()
    const server = await createCompatibilityServer(
      environment.createContext({ name: "server" }),
    )

    const { destinationAccount, sharedSecret } =
      server.generateAddressAndSecret()

    const client = unwrapFailure(
      await createClient({
        context: environment.createContext({ name: "client" }),
        remoteAddress: destinationAccount,
        secret: bufferToUint8Array(sharedSecret),
      }),
    )

    let moneyReceived = 0
    server.on("connection", (connection: Connection) => {
      connection.on("stream", (stream: DataAndMoneyStream) => {
        stream.setReceiveMax(1000)

        stream.on("money", (amount: string) => {
          moneyReceived += Number(amount)
        })
      })
    })

    const stream = client.createStream()

    stream.send(1000n)

    await client.flush()

    expect(moneyReceived).toBe(1000)

    await environment.dispose()
  })
})

describe("Server Compatibility", () => {
  test("should be able to receive money", async ({ expect }) => {
    const environment = createTestEnvironment()

    const server = unwrapFailure(
      await createServer({
        context: environment.createContext({ name: "server" }),
      }),
    )

    let moneyReceived = 0n
    server.on("connection", (connection) => {
      connection.on("stream", (stream) => {
        stream.receive(1000n)

        stream.on("money", (amount) => {
          moneyReceived += amount
        })
      })
    })

    const credentials = server.generateCredentials()

    const client = await createCompatibilityClient(
      environment.createContext({ name: "client" }),
      {
        destinationAccount: credentials.destination,
        sharedSecret: Buffer.from(credentials.secret),
      },
    )

    const stream = client.createStream()

    stream.setSendMax(1000)

    await setTimeout(100)

    await environment.dispose()

    expect(moneyReceived).toBe(1000n)
  })
})
