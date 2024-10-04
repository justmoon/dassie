import type { IlpAllocationScheme } from "@dassie/app-dassie/src/config/computed/ilp-allocation-scheme"
import type { NodeTableStore } from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import type { RoutingTableSignal } from "@dassie/app-dassie/src/routing/signals/routing-table"
import type { InferSignalType } from "@dassie/lib-reactive"

export function sortNodes(
  ilpAllocationScheme: IlpAllocationScheme,
  nodeTable: InferSignalType<typeof NodeTableStore>,
  routingTable: InferSignalType<typeof RoutingTableSignal>,
) {
  return (
    [...nodeTable.values()]
      .map((node) => {
        const routingTableEntry = routingTable.get(
          `${ilpAllocationScheme}.das.${node.nodeId}`,
        )

        if (routingTableEntry?.type !== "peer") {
          return {
            ...node,
            distance: Number.POSITIVE_INFINITY,
            nextHopNodes: [],
          }
        }

        return {
          ...node,
          distance: routingTableEntry.distance,
          nextHopNodes: routingTableEntry.firstHopOptions,
        }
      })
      // Sort first by distance, then by nodeId
      .sort((a, b) => {
        if (a.distance === b.distance) {
          return a.nodeId.localeCompare(b.nodeId)
        }

        return a.distance - b.distance
      })
  )
}
