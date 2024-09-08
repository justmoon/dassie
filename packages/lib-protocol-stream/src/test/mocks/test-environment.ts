import { type Logger, createLogger } from "@dassie/lib-logger"
import { UINT64_MAX } from "@dassie/lib-oer"
import {
  ILDCP_ADDRESS,
  serializeIldcpResponse,
} from "@dassie/lib-protocol-ildcp"
import {
  type IlpEndpoint,
  IlpErrorCode,
  type IlpPacket,
  type IlpPreparePacket,
  IlpType,
  serializeAmountTooLargeData,
} from "@dassie/lib-protocol-ilp"
import {
  type Clock,
  type DisposableScope,
  createMockClock,
  createScope,
  createTopic,
} from "@dassie/lib-reactive"

import type { StreamProtocolContext } from "../../context/context"
import type { CryptoContext } from "../../crypto/context"
import { getPskEnvironment } from "../../crypto/functions"
import { createMockCryptoContext } from "./crypto-context"

interface EnvironmentOptions {
  maxPacketAmount?: bigint
  scope?: DisposableScope | undefined
  logger?: Logger | undefined
  clock?: Clock | undefined
  crypto?: CryptoContext | undefined
}

interface ContextOptions {
  name: string
}

interface TestRoute {
  handler: IlpEndpoint["sendPacket"]
}

interface PreparePacketEvent {
  sender: string
  packet: IlpPreparePacket
}

interface ResponsePacketEvent {
  prepare: IlpPreparePacket
  response: IlpPacket & { type: typeof IlpType.Fulfill | typeof IlpType.Reject }
}

/**
 * Creates a simulated ILP network environment. Basically a tiny connector.
 */
export function createTestEnvironment({
  maxPacketAmount = UINT64_MAX,
  scope = createScope("test-environment"),
  logger = createLogger("das:test:stream"),
  clock = createMockClock(),
  crypto = createMockCryptoContext(),
}: EnvironmentOptions = {}) {
  const routes = new Map<string, TestRoute>()

  const preparePacketTopic = createTopic<PreparePacketEvent>()
  const responsePacketTopic = createTopic<ResponsePacketEvent>()

  logger.debug?.("initializing test environment")

  return {
    createContext: ({ name }: ContextOptions): StreamProtocolContext => {
      const address = `test.${name}`

      async function processPacket(packet: IlpPreparePacket) {
        if (packet.destination.startsWith(address)) {
          throw new Error("Packet addressed to self")
        }

        if (packet.destination === ILDCP_ADDRESS) {
          const ildcpResponse = serializeIldcpResponse({
            address,
            assetScale: 9,
            assetCode: "XRP",
          })

          return {
            type: IlpType.Fulfill,
            data: {
              fulfillment: new Uint8Array(32),
              data: ildcpResponse,
            },
          }
        }

        if (packet.amount > maxPacketAmount) {
          return {
            type: IlpType.Reject,
            data: {
              code: IlpErrorCode.F08_AMOUNT_TOO_LARGE,
              message: "Amount exceeds maximum packet amount",
              triggeredBy: "test.router",
              data: serializeAmountTooLargeData({
                receivedAmount: packet.amount,
                maximumAmount: maxPacketAmount,
              }),
            },
          }
        }

        for (const [address, route] of routes.entries()) {
          if (packet.destination.startsWith(address)) {
            logger.debug?.("routing packet to destination", {
              destination: packet.destination,
            })
            return route.handler(packet)
          }
        }

        throw new Error("Invalid packet destination: " + packet.destination)
      }

      const endpoint: IlpEndpoint = {
        async sendPacket(packet) {
          preparePacketTopic.emit({ sender: address, packet })

          const result = await processPacket(packet)

          responsePacketTopic.emit({ prepare: packet, response: result })

          return result
        },
        handlePackets(handler) {
          if (routes.has(address)) {
            throw new Error("Route already exists")
          }

          routes.set(address, { handler })

          return () => {
            routes.delete(address)
          }
        },
      }

      return {
        crypto,
        logger,
        endpoint,
        scope,
        clock,
      }
    },
    dispose: () => scope.dispose(),
    topics: {
      prepare: preparePacketTopic,
      response: responsePacketTopic,
    },
    getPskEnvironment: (secret: Uint8Array) =>
      getPskEnvironment(crypto, secret),
  }
}
