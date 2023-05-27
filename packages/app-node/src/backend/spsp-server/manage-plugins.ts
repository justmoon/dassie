import type { Plugin } from "ilp-protocol-stream/dist/src/util/plugin-interface"
import { nanoid } from "nanoid"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { nodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { processIncomingPacket } from "../ilp-connector/process-incoming-packet"
import { routingTableSignal } from "../ilp-connector/signals/routing-table"

const logger = createLogger("das:node:manage-plugins")

let nextRequestId = 1
let nextPluginId = 1

type Connection =
  | {
      ilpAddress: string
    }
  | false

type DataHandler = (data: Buffer) => Promise<Buffer>

export const managePlugins = () =>
  createActor((sig) => {
    const nodeIlpAddress = sig.get(nodeIlpAddressSignal)
    const processIncomingPacketActor = sig.use(processIncomingPacket)
    const ilpClientMap = sig.use(routingTableSignal)

    const pluginHandlerMap = new Map<number, DataHandler>()
    const outstandingRequests = new Map<number, (data: Buffer) => void>()

    return {
      createPlugin: (): Plugin => {
        const pluginId = nextPluginId++
        let connection: Connection = false
        let currentHandler: ((data: Buffer) => Promise<Buffer>) | undefined

        let localIlpAddressPart: string

        return {
          connect: () => {
            if (connection) return Promise.resolve()

            do {
              localIlpAddressPart = nanoid(6)
            } while (
              ilpClientMap
                .read()
                .get(`${nodeIlpAddress}.${localIlpAddressPart}`)
            )

            ilpClientMap
              .read()
              .set(`${nodeIlpAddress}.${localIlpAddressPart}`, {
                type: "plugin",
                pluginId,
                localIlpAddressPart,
              })

            connection = {
              ilpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
            }

            return Promise.resolve()
          },
          disconnect: () => {
            if (!connection) return Promise.resolve()

            ilpClientMap
              .read()
              .delete(`${nodeIlpAddress}.${localIlpAddressPart}`)

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
            pluginHandlerMap.set(pluginId, handler)
          },
          deregisterDataHandler: () => {
            currentHandler = undefined
            pluginHandlerMap.delete(pluginId)
          },
        }
      },
      submitPrepare: async ({
        pluginId,
        localIlpAddressPart,
        serializedPacket,
        outgoingRequestId,
      }: {
        pluginId: number
        localIlpAddressPart: string
        serializedPacket: Uint8Array
        outgoingRequestId: number
      }) => {
        const dataHandler = pluginHandlerMap.get(pluginId)

        if (!dataHandler) {
          throw new Error(
            "received prepare packet but there is no registered handler"
          )
        }

        const response = await dataHandler(Buffer.from(serializedPacket))

        processIncomingPacketActor.tell("handle", {
          sourceIlpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
          ledgerAccountPath: "builtin/owner/spsp",
          serializedPacket: response,
          requestId: outgoingRequestId,
        })
      },
      submitResult: ({
        requestId,
        serializedPacket,
      }: {
        requestId: number
        serializedPacket: Uint8Array
      }) => {
        const resolve = outstandingRequests.get(requestId)

        if (!resolve) {
          logger.error(
            "received result packet but there is no outstanding request"
          )
          return
        }

        resolve(Buffer.from(serializedPacket))

        outstandingRequests.delete(requestId)
      },
    }
  })
