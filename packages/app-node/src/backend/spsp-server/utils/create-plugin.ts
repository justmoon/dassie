import type { Plugin } from "ilp-protocol-stream/dist/src/util/plugin-interface"
import { nanoid } from "nanoid"

import assert from "node:assert"

import { type Reactor } from "@dassie/lib-reactive"

import {
  processPacketPrepare,
  processPacketResult,
} from "../../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../../accounting/stores/ledger"
import { IlpType } from "../../ilp-connector/ilp-packet-codec"
import { processIncomingPacket } from "../../ilp-connector/process-incoming-packet"
import { localIlpRoutingTableSignal } from "../../ilp-connector/signals/local-ilp-routing-table"

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

  const ledger = reactor.use(ledgerStore)
  const processIncomingPacketActor = reactor.use(processIncomingPacket)

  return {
    connect: () => {
      if (connection) return Promise.resolve()

      const ilpClientMap = reactor.use(localIlpRoutingTableSignal)

      let localIlpAddressPart: string
      do {
        localIlpAddressPart = nanoid(6)
      } while (ilpClientMap.read().get(localIlpAddressPart))

      ilpClientMap.read().set(localIlpAddressPart, {
        prefix: localIlpAddressPart,
        type: "spsp",
        sendPreparePacket: async ({
          parsedPacket,
          serializedPacket,
          outgoingRequestId: requestId,
        }) => {
          assert(connection)

          if (parsedPacket.amount > 0n) {
            processPacketPrepare(
              ledger,
              "builtin/owner/spsp",
              parsedPacket,
              "outgoing"
            )
          }

          const existingRequest = outstandingRequests.get(requestId)
          if (existingRequest) {
            existingRequest(Buffer.from(serializedPacket))
            outstandingRequests.delete(requestId)
            return
          }

          if (!currentHandler) {
            throw new Error("No handler registered")
          }

          const response = await currentHandler(Buffer.from(serializedPacket))

          processIncomingPacketActor.tell("handle", {
            sourceIlpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
            ledgerAccountPath: "builtin/owner/spsp",
            serializedPacket: response,
            requestId,
          })
        },
        sendResultPacket: ({
          parsedPacket: packet,
          serializedPacket: asUint8Array,
          prepare: {
            parsedPacket: preparePacket,
            incomingRequestId: requestId,
          },
        }) => {
          assert(connection)

          if (preparePacket.amount > 0n) {
            processPacketResult(
              ledger,
              "builtin/owner/spsp",
              preparePacket,
              packet.type === IlpType.Fulfill ? "fulfill" : "reject"
            )
          }

          const existingRequest = outstandingRequests.get(requestId)
          if (!existingRequest) {
            throw new Error("No request found")
          }

          existingRequest(Buffer.from(asUint8Array))
          outstandingRequests.delete(requestId)
        },
      })

      connection = { ilpAddress: `${nodeIlpAddress}.${localIlpAddressPart}` }

      return Promise.resolve()
    },
    disconnect: () => {
      if (!connection) return Promise.resolve()

      reactor
        .use(localIlpRoutingTableSignal)
        .read()
        .delete(connection.ilpAddress)

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

        processIncomingPacketActor.tell("handle", {
          sourceIlpAddress: ilpAddress,
          ledgerAccountPath: "builtin/owner/spsp",
          serializedPacket: data,
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
