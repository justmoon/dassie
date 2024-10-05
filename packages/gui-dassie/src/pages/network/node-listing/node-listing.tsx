import { type ComponentPropsWithoutRef, useMemo } from "react"
import { Link } from "wouter"

import {
  type NodeTableEntry,
  NodeTableStore,
} from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import type { RoutingTableSignal } from "@dassie/app-dassie/src/routing/signals/routing-table"
import type { InferSignalType } from "@dassie/lib-reactive"

import { combine } from "../../../utils/class-helper"
import { rpc } from "../../../utils/rpc"
import { sortNodes } from "../../../utils/sort-nodes"

interface NodeListingProperties extends ComponentPropsWithoutRef<"div"> {
  nodeTable: InferSignalType<typeof NodeTableStore>
  routingTable: InferSignalType<typeof RoutingTableSignal>
}

export function NodeListing({
  nodeTable,
  routingTable,
  className,
  ...remainingProperties
}: NodeListingProperties) {
  const { data: basicState } = rpc.general.getBasicState.useQuery()

  const ilpAllocationScheme = basicState?.ilpAllocationScheme

  const sortedNodeTable = useMemo(() => {
    if (!ilpAllocationScheme) return undefined

    const sortedNodes = sortNodes(ilpAllocationScheme, nodeTable, routingTable)

    const distanceIndexedMap = new Map<number, NodeTableEntry[]>()
    for (const node of sortedNodes) {
      const distance = node.distance
      if (!distanceIndexedMap.has(distance)) {
        distanceIndexedMap.set(distance, [])
      }
      distanceIndexedMap.get(distance)!.push(node)
    }

    return distanceIndexedMap
  }, [ilpAllocationScheme, nodeTable, routingTable])

  if (!basicState || !sortedNodeTable) {
    return <div>Loading...</div>
  }

  return (
    <div
      className={combine("flex flex-col gap-3 px-3", className)}
      {...remainingProperties}
    >
      {[...sortedNodeTable.entries()].map(([distance, nodes]) => (
        <div key={distance} className="flex flex-row gap-2">
          <div className="border-r text-3xl text-muted-foreground pr-2 w-10 text-right">
            {distance === Number.POSITIVE_INFINITY ? "∞" : distance}
          </div>
          <div className="flex-grow">
            {distance === 0 && (
              <h3 className="text-md font-semibold uppercase rounded-full text-muted-foreground">
                This Node
              </h3>
            )}
            {distance === 1 && (
              <h3 className="text-md font-semibold uppercase rounded-full text-muted-foreground">
                Direct Peers
              </h3>
            )}
            {distance === 2 && (
              <h3 className="text-md font-semibold uppercase rounded-full text-muted-foreground">
                Other Nodes
              </h3>
            )}
            {distance === Number.POSITIVE_INFINITY && (
              <h3 className="text-md font-semibold uppercase rounded-full text-muted-foreground">
                No Connection
              </h3>
            )}
            {nodes.map((node) => (
              <Link
                key={node.nodeId}
                href={`/network/${node.nodeId}`}
                className="block hover:bg-accent cursor-pointer"
              >
                <h3 className="text-lg font-bold">{node.nodeId}</h3>
                <p className="text-sm text-muted-foreground">
                  {node.linkState?.url ?? "no link state"}
                </p>
              </Link>
            ))}
          </div>
        </div>
      ))}
    </div>
  )
}
