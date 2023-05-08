import type { Plugin } from "ilp-protocol-stream/dist/src/util/plugin-interface"
import { nanoid } from "nanoid"

import { type Reactor } from "@dassie/lib-reactive"

import { ilpRoutingTableSignal } from "../../ilp-connector/signals/ilp-routing-table"
import { incomingIlpPacketTopic } from "../../ilp-connector/topics/incoming-ilp-packet"

let nextRequestId = 1

const outstandingRequests = new Map<number, (data: Buffer) => void>()

type Connection =
  | {
      ilpAddress: string
    }
  | false

export const createPlugin = (
  reactor: Reactor,
  nodeIlpAddress: string
): Plugin => {
  let connection: Connection = false
  let currentHandler: ((data: Buffer) => Promise<Buffer>) | undefined

  return {
    connect: () => {
      if (connection) return Promise.resolve()

      const ilpClientMap = reactor.use(ilpRoutingTableSignal)

      let ilpAddress: string
      do {
        ilpAddress = `${nodeIlpAddress}.${nanoid(6)}`
      } while (ilpClientMap.read().get(ilpAddress))

      ilpClientMap.read().set(ilpAddress, {
        prefix: ilpAddress,
        type: "spsp",
        sendPacket: async ({ asUint8Array, requestId }) => {
          const existingRequest = outstandingRequests.get(requestId)
          if (existingRequest) {
            existingRequest(Buffer.from(asUint8Array))
            outstandingRequests.delete(requestId)
            return
          }

          if (!currentHandler) {
            throw new Error("No handler registered")
          }

          const response = await currentHandler(Buffer.from(asUint8Array))

          reactor.use(incomingIlpPacketTopic).emitPacket({
            packet: response,
            source: ilpAddress,
            requestId,
          })
        },
      })

      connection = { ilpAddress }

      return Promise.resolve()
    },
    disconnect: () => {
      if (!connection) return Promise.resolve()

      reactor.use(ilpRoutingTableSignal).read().delete(connection.ilpAddress)

      connection = false

      return Promise.resolve()
    },
    isConnected: () => {
      return !!connection
    },
    sendData: (data: Buffer) => {
      if (!connection) {
        throw new Error("Plugin is not connected")
      }

      const { ilpAddress } = connection

      const resultPromise = new Promise<Buffer>((resolve) => {
        const requestId = nextRequestId++
        outstandingRequests.set(requestId, resolve)
        reactor.use(incomingIlpPacketTopic).emitPacket({
          packet: data,
          source: ilpAddress,
          requestId,
        })
      })

      return resultPromise
    },
    registerDataHandler: (handler: (data: Buffer) => Promise<Buffer>) => {
      if (currentHandler) {
        throw new Error("Cannot register multiple handlers")
      }

      currentHandler = handler
    },
    deregisterDataHandler: () => {
      currentHandler = undefined
    },
  }
}
