import { createLogger } from "@dassie/lib-logger"
import {
  ILDCP_ADDRESS,
  serializeIldcpResponse,
} from "@dassie/lib-protocol-ildcp"
import { type IlpEndpoint, IlpType } from "@dassie/lib-protocol-ilp"
import { createScope } from "@dassie/lib-reactive"

import type { StreamProtocolContext } from "../../context/context"
import { createMockCryptoContext } from "./crypto-context"

interface ContextOptions {
  name: string
}

interface TestRoute {
  handler: IlpEndpoint["sendPacket"]
}

/**
 * Creates a simulated ILP network environment. Basically a tiny connector.
 */
export function createTestEnvironment() {
  const routes = new Map<string, TestRoute>()
  const scope = createScope("test-environment")

  return {
    createContext: ({ name }: ContextOptions): StreamProtocolContext => {
      const address = `test.${name}`

      const endpoint: IlpEndpoint = {
        async sendPacket(packet) {
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
                fulfillment: Buffer.alloc(32),
                data: ildcpResponse,
              },
            }
          }

          for (const [address, route] of routes.entries()) {
            if (packet.destination.startsWith(address)) {
              return route.handler(packet)
            }
          }

          throw new Error("Invalid packet destination: " + packet.destination)
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
        crypto: createMockCryptoContext(),
        logger: createLogger("das:test:stream"),
        endpoint,
        scope,
        getDateNow() {
          return new Date("2024-08-29T10:44:29.380Z").valueOf()
        },
      }
    },
    dispose: () => scope.dispose(),
  }
}
