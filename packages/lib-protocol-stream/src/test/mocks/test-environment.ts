import { type Logger, createLogger } from "@dassie/lib-logger"
import { UINT64_MAX } from "@dassie/lib-oer"
import {
  ILDCP_ADDRESS,
  serializeIldcpResponse,
} from "@dassie/lib-protocol-ildcp"
import {
  type IlpEndpoint,
  IlpErrorCode,
  type IlpPreparePacket,
  type IlpResponsePacket,
  IlpType,
  serializeAmountTooLargeData,
} from "@dassie/lib-protocol-ilp"
import {
  type Clock,
  type Crypto,
  type DisposableScope,
  type Signal,
  createMockClock,
  createMockDeterministicCrypto,
  createScope,
  createSignal,
  createTopic,
  sampleLogNormalDistribution,
} from "@dassie/lib-reactive"
import { createCrypto } from "@dassie/lib-reactive-io"

import type { StreamProtocolContext } from "../../context/context"
import { DEFAULT_POLICY, type StreamPolicy } from "../../context/policy"
import { getPskEnvironment } from "../../crypto/functions"

interface EnvironmentOptions {
  // Environment options
  maxPacketAmount?: bigint | undefined
  latency?: number | undefined
  jitter?: number | undefined
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

  startingBalance?: bigint
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
  prepareSender: string
  prepare: IlpPreparePacket
  response: IlpResponsePacket
}

/**
 * Creates a simulated ILP network environment. Basically a tiny connector.
 */
export function createTestEnvironment({
  maxPacketAmount = UINT64_MAX,
  latency = 0,
  jitter = 0,
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

  const latencyGenerator = sampleLogNormalDistribution(
    crypto,
    Math.log(latency),
    jitter,
  )

  logger.debug?.("initializing test environment")

  return {
    createContext: ({
      name,
      unitsPerToken = 1_000_000n,
      startingBalance = 0n,
    }: ContextOptions): StreamProtocolContext & {
      _test: { balance: Signal<bigint> }
    } => {
      const address = `test.${name}`
      const balance = createSignal(startingBalance)

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
            clock.setTimeout(() => resolve(), latencyGenerator.next().value),
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

        return {
          type: IlpType.Reject,
          data: {
            code: IlpErrorCode.F02_UNREACHABLE,
            message: "Unknown destination",
            triggeredBy: "test.router",
            data: new Uint8Array(),
          },
        }
      }

      const endpoint: IlpEndpoint = {
        async sendPacket(packet) {
          preparePacketTopic.emit({ sender: address, packet })

          balance.update((balance) => balance - packet.amount)

          const result = await processPacket(packet)

          if (result.type === IlpType.Reject) {
            balance.update((balance) => balance + packet.amount)
          }

          responsePacketTopic.emit({
            prepareSender: address,
            prepare: packet,
            response: result,
          })

          return result
        },
        handlePackets(handler) {
          if (routes.has(address)) {
            throw new Error("Route already exists")
          }

          routes.set(address, {
            handler: async (packet) => {
              const result = await handler(packet)

              if (result.type === IlpType.Fulfill) {
                balance.update((balance) => balance + packet.amount)
              }

              return result
            },
            unitsPerToken,
          })

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

        _test: {
          balance,
        },
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
