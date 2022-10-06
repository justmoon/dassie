import Denque from "denque"

import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"
import { assertDefined } from "@dassie/lib-type-utils"

import { configSignal } from "../config"
import { ilpRoutingTableSignal } from "../ilp-connector/signals/ilp-routing-table"
import { peerMessage } from "./peer-schema"
import { outgoingPeerMessageBufferTopic } from "./send-peer-messages"
import { nodeTableStore } from "./stores/node-table"
import { RoutingTableEntry, routingTableStore } from "./stores/routing-table"

const logger = createLogger("das:node:calculate-routes")

interface NodeInfoEntry {
  level: number
  parents: string[]
}

/**
 * This effect generates an Even-Shiloach tree which is then condensed into a routing table. The routing table contains all possible first hops which are on one of the shortest paths to the target node.
 */
export const calculateRoutes = (sig: EffectContext) => {
  const subnetId = sig.get(configSignal, (config) => config.subnetId)
  const ownNodeId = sig.get(configSignal, ({ nodeId }) => nodeId)
  const nodeTable = sig.get(nodeTableStore)

  const queue = new Denque([ownNodeId])
  const queueSet = new Set([ownNodeId])

  const nodeInfoMap = new Map<string, NodeInfoEntry>()
  nodeInfoMap.set(ownNodeId, { level: 0, parents: [] })

  let currentNodeId
  while ((currentNodeId = queue.shift()) != undefined) {
    queueSet.delete(currentNodeId)

    const currentNode = nodeTable.get(currentNodeId)
    if (currentNode == undefined) {
      // We only route to nodes in our node table, so we will ignore this destination for now
      continue
    }

    const currentNodeInfo = nodeInfoMap.get(currentNodeId)
    assertDefined(currentNodeInfo)

    for (const neighborId of currentNode.neighbors) {
      const neighborInfo = nodeInfoMap.get(neighborId)

      if (neighborInfo) {
        if (neighborInfo.level > currentNodeInfo.level + 1) {
          // this is a new shortest path to this node
          neighborInfo.level = currentNodeInfo.level + 1
          neighborInfo.parents = [currentNodeId]
        } else if (neighborInfo.level === currentNodeInfo.level + 1) {
          // this is an alternative path of equal length
          neighborInfo.parents.push(currentNodeId)
        } else {
          // this is a longer path than the one(s) we already have, ignore
        }
      } else {
        // this is a node we haven't reached before
        const newNeighborInfo = {
          level: currentNodeInfo.level + 1,
          parents: [currentNodeId],
        }
        nodeInfoMap.set(neighborId, newNeighborInfo)

        queue.push(neighborId)
        queueSet.add(neighborId)
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

    routes.set(nodeId, {
      distance: nodeInfo.level,
      firstHopOptions: [...parents],
    })

    const ilpAddress = `g.das.${subnetId}.${nodeId}`
    ilpRoutingTable.set(ilpAddress, {
      prefix: ilpAddress,
      type: "peer",
      sendPacket: (packet) => {
        const peerMessageSerializationResult = peerMessage.serialize({
          interledgerPacket: {
            signed: {
              source: `g.das.${subnetId}.${ownNodeId}`,
              requestId: packet.requestId,
              packet: packet.packet,
            },
          },
        })

        if (!peerMessageSerializationResult.success) {
          logger.error("Unable to serialize peer message", {
            error: peerMessageSerializationResult.failure,
          })
          return
        }

        sig.use(outgoingPeerMessageBufferTopic).emit({
          destination: nodeId,
          message: peerMessageSerializationResult.value,
        })
      },
    })
  }

  sig.onCleanup(() => {
    for (const nodeId of nodeInfoMap.keys()) {
      const ilpAddress = `g.das.${subnetId}.${nodeId}`
      ilpRoutingTable.delete(ilpAddress)
    }
  })

  sig.use(routingTableStore).write(routes)
}
