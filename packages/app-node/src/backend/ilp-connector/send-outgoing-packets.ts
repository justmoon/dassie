import type { Promisable } from "type-fest"

import { createActor } from "@dassie/lib-reactive"

import { configSignal } from ".."
import { nodeIdSignal } from "./computed/node-id"
import { globalIlpRoutingTableSignal } from "./signals/global-ilp-routing-table"
import { localIlpRoutingTableSignal } from "./signals/local-ilp-routing-table"
import {
  PreparedIlpPacketEvent,
  preparedIlpPacketTopic,
} from "./topics/prepared-ilp-packet"
import {
  ResolvedIlpPacketEvent,
  resolvedIlpPacketTopic,
} from "./topics/resolved-ilp-packet"

export interface IlpClientInfo {
  prefix: string
  type: string
  sendPreparePacket: (event: PreparedIlpPacketEvent) => Promisable<void>
  sendResultPacket: (event: ResolvedIlpPacketEvent) => Promisable<void>
}

export const sendOutgoingPackets = () =>
  createActor((sig) => {
    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const nodeId = sig.get(nodeIdSignal)
    const globalIlpClientMap = sig.use(globalIlpRoutingTableSignal)
    const localIlpClientMap = sig.use(localIlpRoutingTableSignal)

    const generalPrefix = `${ilpAllocationScheme}.das.`

    const getLocalPart = (destination: string): string | false => {
      if (!destination.startsWith(generalPrefix)) {
        return false
      }

      const nodeIdStartIndex = destination.indexOf(".", generalPrefix.length)

      if (nodeIdStartIndex === -1) {
        return false
      }

      const nodeIdEndIndex = destination.indexOf(".", nodeIdStartIndex + 1)

      const destinationNodeId = destination.slice(
        nodeIdStartIndex + 1,
        nodeIdEndIndex === -1 ? undefined : nodeIdEndIndex
      )

      return destinationNodeId === nodeId
        ? destination.slice(nodeIdEndIndex + 1)
        : false
    }

    const getClient = (destination: string): IlpClientInfo | undefined => {
      const localPart = getLocalPart(destination)

      return localPart === false
        ? globalIlpClientMap.read().lookup(destination)
        : localIlpClientMap.read().lookup(localPart)
    }

    sig.on(preparedIlpPacketTopic, async (event) => {
      const client = getClient(event.parsedPacket.destination)

      if (!client) {
        throw new Error(
          `No routing table entry found for ${event.parsedPacket.destination}`
        )
      }

      await client.sendPreparePacket(event)
    })

    sig.on(resolvedIlpPacketTopic, async (event) => {
      const client = getClient(event.prepare.sourceIlpAddress)

      if (!client) {
        throw new Error(
          `No routing table entry found for ${event.prepare.sourceIlpAddress}`
        )
      }

      await client.sendResultPacket(event)
    })
  })
