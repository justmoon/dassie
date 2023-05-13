import type { Plugin } from "ilp-protocol-stream/dist/src/util/plugin-interface"
import { nanoid } from "nanoid"

import assert from "node:assert"

import { type Reactor } from "@dassie/lib-reactive"
import { isFailure } from "@dassie/lib-type-utils"

import {
  processPacketPrepare,
  processPacketResult,
} from "../../accounting/functions/process-interledger-packet"
import { ledgerStore } from "../../accounting/stores/ledger"
import { IlpType } from "../../ilp-connector/ilp-packet-codec"
import { localIlpRoutingTableSignal } from "../../ilp-connector/signals/local-ilp-routing-table"
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

  const ledger = reactor.use(ledgerStore)
  const incomingIlpPacketTopicValue = reactor.use(incomingIlpPacketTopic)

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
        sendPacket: async ({ packet, asUint8Array, requestId }) => {
          assert(connection)

          switch (packet.type) {
            case IlpType.Prepare: {
              if (packet.amount > 0n) {
                processPacketPrepare(ledger, "owner/spsp", packet, "outgoing")
              }
              break
            }
            case IlpType.Fulfill: {
              if (packet.prepare.amount > 0n) {
                processPacketResult(
                  ledger,
                  "owner/spsp",
                  packet.prepare,
                  "fulfill"
                )
              }
              break
            }
            case IlpType.Reject: {
              if (packet.prepare.amount > 0n) {
                processPacketResult(
                  ledger,
                  "owner/spsp",
                  packet.prepare,
                  "reject"
                )
              }
              break
            }
          }

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

          const incomingPacketEvent = incomingIlpPacketTopicValue.prepareEvent({
            packet: response,
            source: `${nodeIlpAddress}.${localIlpAddressPart}`,
            requestId,
          })

          // TODO: refactor to simplify/unify the processing of packets coming in and going out
          {
            const { packet } = incomingPacketEvent

            switch (packet.type) {
              case IlpType.Prepare: {
                if (packet.amount > 0n) {
                  const result = processPacketPrepare(
                    ledger,
                    "owner/spsp",
                    packet,
                    "incoming"
                  )
                  if (isFailure(result)) {
                    // TODO: reject packet
                    throw new Error(
                      `failed to create transfer, invalid ${result.whichAccount} account: ${result.accountPath}`
                    )
                  }
                }
                break
              }
              case IlpType.Fulfill: {
                if (packet.prepare.amount > 0n) {
                  processPacketResult(
                    ledger,
                    "owner/spsp",
                    packet.prepare,
                    "fulfill"
                  )
                }
                break
              }
              case IlpType.Reject: {
                if (packet.prepare.amount > 0n) {
                  processPacketResult(
                    ledger,
                    "owner/spsp",
                    packet.prepare,
                    "reject"
                  )
                }
                break
              }
            }
          }

          reactor.use(incomingIlpPacketTopic).emit(incomingPacketEvent)
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

        const incomingPacketEvent = incomingIlpPacketTopicValue.prepareEvent({
          packet: data,
          source: ilpAddress,
          requestId,
        })

        const { packet } = incomingPacketEvent

        switch (packet.type) {
          case IlpType.Prepare: {
            if (packet.amount > 0n) {
              const result = processPacketPrepare(
                ledger,
                "owner/spsp",
                packet,
                "incoming"
              )
              if (isFailure(result)) {
                // TODO: reject packet
                throw new Error(
                  `failed to create transfer, invalid ${result.whichAccount} account: ${result.accountPath}`
                )
              }
            }
            break
          }
          case IlpType.Fulfill: {
            if (packet.prepare.amount > 0n) {
              processPacketResult(
                ledger,
                "owner/spsp",
                packet.prepare,
                "fulfill"
              )
            }
            break
          }
          case IlpType.Reject: {
            if (packet.prepare.amount > 0n) {
              processPacketResult(
                ledger,
                "owner/spsp",
                packet.prepare,
                "reject"
              )
            }
            break
          }
        }

        reactor.use(incomingIlpPacketTopic).emit(incomingPacketEvent)
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
