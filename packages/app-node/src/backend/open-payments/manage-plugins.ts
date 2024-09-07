import type { Plugin } from "ilp-protocol-stream/dist/src/util/plugin-interface"
import { nanoid } from "nanoid"

import { parseIlpPacket } from "@dassie/lib-protocol-ilp"
import { createActor } from "@dassie/lib-reactive"

import { OwnerLedgerIdSignal } from "../accounting/signals/owner-ledger-id"
import type {
  DassieActorContext,
  DassieReactor,
} from "../base/types/dassie-base"
import { NodeIlpAddressSignal } from "../ilp-connector/computed/node-ilp-address"
import { ProcessPacket } from "../ilp-connector/functions/process-packet"
import type { PluginEndpointInfo } from "../ilp-connector/senders/send-plugin-packets"
import { payment as logger } from "../logger/instances"
import { RoutingTableSignal } from "../routing/signals/routing-table"

type Connection = PluginEndpointInfo | false

type DataHandler = (data: Buffer) => Promise<Buffer>

export const ManagePluginsActor = (reactor: DassieReactor) => {
  const routingTable = reactor.use(RoutingTableSignal)
  const ownerLedgerIdSignal = reactor.use(OwnerLedgerIdSignal)

  let nextRequestId = 1
  let nextPluginId = 1

  return createActor((sig: DassieActorContext) => {
    const nodeIlpAddress = sig.readAndTrack(NodeIlpAddressSignal)
    const processPacket = reactor.use(ProcessPacket)

    const pluginHandlerMap = new Map<number, DataHandler>()
    const outstandingRequests = new Map<
      number | string,
      (data: Buffer) => void
    >()

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
              routingTable
                .read()
                .get(`${nodeIlpAddress}.${localIlpAddressPart}`)
            )

            connection = {
              type: "plugin",
              pluginId,
              localIlpAddressPart,
              ilpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
              accountPath: `${ownerLedgerIdSignal.read()}:equity/owner`,
            }

            routingTable
              .read()
              .set(`${nodeIlpAddress}.${localIlpAddressPart}`, {
                type: "fixed",
                destination: connection,
              })

            return Promise.resolve()
          },
          disconnect: () => {
            if (!connection) return Promise.resolve()

            routingTable
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

            const sourceEndpointInfo = connection

            const resultPromise = new Promise<Buffer>((resolve) => {
              const requestId = nextRequestId++
              outstandingRequests.set(requestId, resolve)

              processPacket({
                sourceEndpointInfo,
                serializedPacket: data,
                parsedPacket: parseIlpPacket(data),
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
            "received prepare packet but there is no registered handler",
          )
        }

        const response = await dataHandler(Buffer.from(serializedPacket))

        const sourceEndpointInfo: PluginEndpointInfo = {
          type: "plugin",
          pluginId,
          localIlpAddressPart,
          ilpAddress: `${nodeIlpAddress}.${localIlpAddressPart}`,
          accountPath: `${ownerLedgerIdSignal.read()}:equity/owner`,
        }

        processPacket({
          sourceEndpointInfo,
          serializedPacket: response,
          parsedPacket: parseIlpPacket(response),
          requestId: outgoingRequestId,
        })
      },
      submitResult: ({
        requestId,
        serializedPacket,
      }: {
        requestId: number | string
        serializedPacket: Uint8Array
      }) => {
        const resolve = outstandingRequests.get(requestId)

        if (!resolve) {
          logger.error(
            "received result packet but there is no outstanding request",
          )
          return
        }

        resolve(Buffer.from(serializedPacket))

        outstandingRequests.delete(requestId)
      },
    }
  })
}
