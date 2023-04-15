import Denque from "denque"

import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"
import { assertDefined } from "@dassie/lib-type-utils"

import { peerBalanceMapStore } from "../balances/stores/peer-balance-map"
import { configSignal } from "../config"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { IlpType } from "../ilp-connector/ilp-packet-codec"
import { ilpRoutingTableSignal } from "../ilp-connector/signals/ilp-routing-table"
import subnetModules from "../subnets/modules"
import { sendPeerMessage } from "./actions/send-peer-message"
import type { PerSubnetParameters } from "./run-per-subnet-effects"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { type NodeTableKey, nodeTableStore } from "./stores/node-table"
import {
  type RoutingTableEntry,
  routingTableStore,
} from "./stores/routing-table"

const logger = createLogger("das:node:calculate-routes")

interface NodeInfoEntry {
  level: number
  parents: string[]
}

/**
 * This effect generates an Even-Shiloach tree which is then condensed into a routing table. The routing table contains all possible first hops which are on one of the shortest paths to the target node.
 */
export const calculateRoutes = () =>
  createActor((sig, parameters: PerSubnetParameters) => {
    const { subnetId } = parameters

    const { ilpAllocationScheme } = sig.getKeys(configSignal, [
      "ilpAllocationScheme",
    ])
    const ownNodeId = sig.get(nodeIdSignal)
    const nodeTable = sig.get(nodeTableStore)

    const ownNodeTableEntry = nodeTable.get(`${subnetId}.${ownNodeId}`)

    const queue = new Denque([ownNodeTableEntry])
    const queueSet = new Set([ownNodeId])

    const nodeInfoMap = new Map<string, NodeInfoEntry>()
    nodeInfoMap.set(ownNodeId, { level: 0, parents: [] })

    let currentNode
    while ((currentNode = queue.shift()) != undefined) {
      queueSet.delete(currentNode.nodeId)

      const currentNodeInfo = nodeInfoMap.get(currentNode.nodeId)
      assertDefined(currentNodeInfo)

      for (const neighborId of currentNode.linkState.neighbors) {
        const neighborInfo = nodeInfoMap.get(neighborId)

        if (neighborInfo) {
          if (neighborInfo.level > currentNodeInfo.level + 1) {
            // this is a new shortest path to this node
            neighborInfo.level = currentNodeInfo.level + 1
            neighborInfo.parents = [currentNode.nodeId]
          } else if (neighborInfo.level === currentNodeInfo.level + 1) {
            // this is an alternative path of equal length
            neighborInfo.parents.push(currentNode.nodeId)
          } else {
            // this is a longer path than the one(s) we already have, ignore
          }
        } else {
          // this is a node we haven't reached before
          const newNeighborInfo = {
            level: currentNodeInfo.level + 1,
            parents: [currentNode.nodeId],
          }
          nodeInfoMap.set(neighborId, newNeighborInfo)

          const neighbor = nodeTable.get(`${subnetId}.${neighborId}`)

          if (neighbor) {
            queue.push(neighbor)
            queueSet.add(neighborId)
          } else {
            // We came across a node in the graph that we don't know. Let's ask someone about it.
            sig
              .use(nodeDiscoveryQueueStore)
              .addNode(`${subnetId}.${neighborId}`, currentNode.nodeId)
          }
        }
      }
    }

    const routes = new Map<string, RoutingTableEntry>()
    const ilpRoutingTable = sig.get(ilpRoutingTableSignal)
    for (const [nodeId, nodeInfo] of nodeInfoMap.entries()) {
      let level = nodeInfo.level
      let parents = new Set(nodeInfo.parents)

      while (--level > 1) {
        parents = new Set(
          [...parents].flatMap(
            (parentId) => nodeInfoMap.get(parentId)?.parents ?? []
          )
        )
      }

      const firstHopOptions = nodeInfo.level === 1 ? [nodeId] : [...parents]

      routes.set(nodeId, {
        distance: nodeInfo.level,
        firstHopOptions,
      })

      if (nodeInfo.level > 0) {
        const ilpAddress = `${ilpAllocationScheme}.das.${subnetId}.${nodeId}`
        ilpRoutingTable.set(ilpAddress, {
          prefix: ilpAddress,
          type: "peer",
          sendPacket: ({ packet, asUint8Array, requestId }) => {
            const nextHop = firstHopOptions[0]!

            const subnetModule = subnetModules[subnetId]

            if (!subnetModule) {
              throw new Error(`No subnet module found for subnet ${subnetId}`)
            }

            const balanceMap = sig.use(peerBalanceMapStore)

            const peerKey: NodeTableKey = `${subnetId}.${nextHop}`
            switch (packet.type) {
              case IlpType.Prepare: {
                balanceMap.handleOutgoingPrepared(peerKey, packet.amount)
                break
              }
              case IlpType.Fulfill: {
                balanceMap.handleIncomingFulfilled(
                  peerKey,
                  packet.prepare.amount
                )
                break
              }
              case IlpType.Reject: {
                balanceMap.handleIncomingRejected(
                  peerKey,
                  packet.prepare.amount
                )
                break
              }
            }

            logger.debug("sending ilp packet", { nextHop })

            sig.use(sendPeerMessage).tell({
              subnet: subnetId,
              destination: nextHop,
              message: {
                type: "interledgerPacket",
                value: {
                  signed: {
                    requestId: requestId,
                    packet: asUint8Array,
                  },
                },
              },
            })
          },
        })
      }
    }

    sig.onCleanup(() => {
      for (const nodeId of nodeInfoMap.keys()) {
        const ilpAddress = `${ilpAllocationScheme}.das.${subnetId}.${nodeId}`
        ilpRoutingTable.delete(ilpAddress)
      }
    })

    sig.use(routingTableStore).write(routes)
  })
