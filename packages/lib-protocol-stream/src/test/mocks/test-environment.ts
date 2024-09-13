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
  type Crypto,
  type DisposableScope,
  createMockClock,
  createMockDeterministicCrypto,
  createScope,
  createTopic,
} from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

import type { StreamProtocolContext } from "../../context/context"
import { DEFAULT_POLICY, type StreamPolicy } from "../../context/policy"
import { getPskEnvironment } from "../../crypto/functions"

interface EnvironmentOptions {
  // Environment options
  maxPacketAmount?: bigint | undefined
  latency?: number | undefined
  maxPacketsInFlight?: number | undefined

  // Override context
  scope?: DisposableScope | undefined
  logger?: Logger | undefined
  clock?: Clock | undefined
  crypto?: Crypto | undefined
  policy?: StreamPolicy | undefined
}

interface ContextOptions {
  name: string

  /**
   * How many internal accounting units represent one unit on this endpoint.
   *
   * Defaults to 1_000_000n
   */
  unitsPerToken?: bigint
}

interface TestRoute {
  handler: IlpEndpoint["sendPacket"]
  unitsPerToken: bigint
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
  latency = 0,
  maxPacketsInFlight = Infinity,
  scope = createScope("test-environment"),
  logger = createLogger("das:test:stream"),
  crypto = createMockDeterministicCrypto(createCrypto()),
  clock = createMockClock(),
  policy = DEFAULT_POLICY,
}: EnvironmentOptions = {}) {
  const routes = new Map<string, TestRoute>()

  const preparePacketTopic = createTopic<PreparePacketEvent>()
  const responsePacketTopic = createTopic<ResponsePacketEvent>()

  logger.debug?.("initializing test environment")

  return {
    createContext: ({
      name,
      unitsPerToken = 1_000_000n,
    }: ContextOptions): StreamProtocolContext => {
      const address = `test.${name}`

      let packetsInFlight = 0

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

        if (latency > 0) {
          await new Promise<void>((resolve) =>
            clock.setTimeout(() => resolve(), latency),
          )
        }

        if (packetsInFlight >= maxPacketsInFlight) {
          return {
            type: IlpType.Reject,
            data: {
              code: IlpErrorCode.T03_CONNECTOR_BUSY,
              message: "Too many packets in flight",
              triggeredBy: "test.router",
              data: new Uint8Array(),
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

        const internalAmount = packet.amount * unitsPerToken

        for (const [address, route] of routes.entries()) {
          if (packet.destination.startsWith(address)) {
            logger.debug?.("routing packet to destination", {
              destination: packet.destination,
            })

            const outputPacket = {
              ...packet,
              amount: internalAmount / route.unitsPerToken,
            }

            packetsInFlight++
            return route.handler(outputPacket).finally(() => {
              packetsInFlight--
            })
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

          routes.set(address, { handler, unitsPerToken })

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
        policy,
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
