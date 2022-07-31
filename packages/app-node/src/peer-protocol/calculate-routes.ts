import Denque from "denque"

import { EffectContext, createStore } from "@xen-ilp/lib-reactive"
import { assertDefined } from "@xen-ilp/lib-type-utils"

import { configStore } from "../config"
import { nodeTableStore } from "./stores/node-table"

interface NodeInfoEntry {
  level: number
  parents: string[]
}

interface RoutingTableEntry {
  distance: number
  firstHopOptions: string[]
}

export const routingTableStore = () =>
  createStore(new Map<string, RoutingTableEntry>())

/**
 * This effect generates an Even-Shiloach tree which is then condensed into a routing table. The routing table contains all possible first hops which are on one of the shortest paths to the target node.
 */
export const calculateRoutes = (sig: EffectContext) => {
  const ownNodeId = sig.get(configStore, ({ nodeId }) => nodeId)
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
  }

  sig.emit(routingTableStore, () => routes)
}
