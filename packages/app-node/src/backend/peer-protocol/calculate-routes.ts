import Denque from "denque"

import { createActor } from "@dassie/lib-reactive"
import { assertDefined } from "@dassie/lib-type-utils"

import { environmentConfigSignal } from "../config/environment-config"
import { nodeIdSignal } from "../ilp-connector/computed/node-id"
import { routingTableSignal } from "../ilp-connector/signals/routing-table"
import { nodeDiscoveryQueueStore } from "./stores/node-discovery-queue"
import { nodeTableStore } from "./stores/node-table"
import { NodeId } from "./types/node-id"

interface NodeInfoEntry {
  level: number
  parents: NodeId[]
}

/**
 * This actor generates an Even-Shiloach tree which is then condensed into a routing table. The routing table contains all possible first hops which are on one of the shortest paths to the target node.
 */
export const calculateRoutes = () =>
  createActor((sig) => {
    const { ilpAllocationScheme } = sig.getKeys(environmentConfigSignal, [
      "ilpAllocationScheme",
    ])
    const ownNodeId = sig.get(nodeIdSignal)
    const nodeTable = sig.get(nodeTableStore)

    const ownNodeTableEntry = nodeTable.get(ownNodeId)

    const queue = new Denque([ownNodeTableEntry])
    const queueSet = new Set([ownNodeId])

    const nodeInfoMap = new Map<NodeId, NodeInfoEntry>()
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

          const neighbor = nodeTable.get(neighborId)

          if (neighbor) {
            queue.push(neighbor)
            queueSet.add(neighborId)
          } else {
            // We came across a node in the graph that we don't know. Let's ask someone about it.
            sig
              .use(nodeDiscoveryQueueStore)
              .addNode(neighborId, currentNode.nodeId)
          }
        }
      }
    }

    const ilpRoutingTable = sig.get(routingTableSignal)
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

      const ilpAddress = `${ilpAllocationScheme}.das.${nodeId}`

      ilpRoutingTable.set(ilpAddress, {
        type: "peer",
        firstHopOptions,
        distance: nodeInfo.level,
      })
    }

    sig.onCleanup(() => {
      for (const nodeId of nodeInfoMap.keys()) {
        const ilpAddress = `${ilpAllocationScheme}.das.${nodeId}`
        ilpRoutingTable.delete(ilpAddress)
      }
    })
  })
