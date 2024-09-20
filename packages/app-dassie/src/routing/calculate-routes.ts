import Denque from "denque"

import { type Reactor, createActor } from "@dassie/lib-reactive"
import { assertDefined } from "@dassie/lib-type-utils"

import { IlpAllocationSchemeSignal } from "../config/computed/ilp-allocation-scheme"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import type { IlpAddress } from "../ilp-connector/types/ilp-address"
import { NodeTableStore } from "../peer-protocol/stores/node-table"
import type { NodeId } from "../peer-protocol/types/node-id"
import { RoutingTableSignal } from "./signals/routing-table"

interface NodeInfoEntry {
  level: number
  parents: NodeId[]
}

/**
 * This actor generates an Even-Shiloach tree which is then condensed into a routing table. The routing table contains all possible first hops which are on one of the shortest paths to the target node.
 */
export const CalculateRoutesActor = (reactor: Reactor) => {
  const routingTableSignal = reactor.use(RoutingTableSignal)

  return createActor((sig) => {
    const ilpAllocationScheme = sig.readAndTrack(IlpAllocationSchemeSignal)
    const ownNodeId = sig.readAndTrack(NodeIdSignal)
    const nodeTable = sig.readAndTrack(NodeTableStore)

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

      const neighbors = currentNode.linkState?.neighbors ?? []
      for (const neighborId of neighbors) {
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
          }
        }
      }
    }

    const routingTable = routingTableSignal.read()
    for (const [nodeId, nodeInfo] of nodeInfoMap.entries()) {
      let level = nodeInfo.level
      let parents = new Set(nodeInfo.parents)

      while (--level > 1) {
        parents = new Set(
          [...parents].flatMap(
            (parentId) => nodeInfoMap.get(parentId)?.parents ?? [],
          ),
        )
      }

      const firstHopOptions = nodeInfo.level === 1 ? [nodeId] : [...parents]

      const ilpAddress: IlpAddress = `${ilpAllocationScheme}.das.${nodeId}`

      routingTable.set(ilpAddress, {
        type: "peer",
        firstHopOptions,
        distance: nodeInfo.level,
      })
    }

    // Notify the routing table signal that the routing table has changed
    routingTableSignal.write(routingTable)

    sig.onCleanup(() => {
      for (const nodeId of nodeInfoMap.keys()) {
        const ilpAddress: IlpAddress = `${ilpAllocationScheme}.das.${nodeId}`
        routingTable.delete(ilpAddress)
      }
    })
  })
}
