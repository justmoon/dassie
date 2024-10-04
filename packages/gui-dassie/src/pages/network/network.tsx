import { useEffect, useMemo, useReducer } from "react"

import { NodeTableStore } from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import type { InferSignalType } from "@dassie/lib-reactive"
import {
  useRemoteSignal,
  useRemoteStore,
} from "@dassie/lib-reactive-rpc/client"

import { rpc } from "../../utils/rpc"
import { sortNodes } from "../../utils/sort-nodes"
import { type GraphData, NetworkGraph } from "./network-graph/network-graph"

export function NetworkPage() {
  const nodeTable = useRemoteStore(
    rpc.network.subscribeToNodeTableStore,
    NodeTableStore,
  )
  const routingTable = useRemoteSignal(rpc.debug.subscribeRoutingTable)

  const { data: basicState } = rpc.general.getBasicState.useQuery()
  const { data: ilpAllocationScheme } =
    rpc.general.getAllocationScheme.useQuery(undefined)

  const sortedNodeTable = useMemo(() => {
    if (!ilpAllocationScheme || !routingTable) return undefined

    return sortNodes(ilpAllocationScheme, nodeTable, routingTable)
  }, [ilpAllocationScheme, nodeTable, routingTable])

  const [graphData, dispatchGraphData] = useReducer(
    (
      previousGraphData: GraphData,
      action: InferSignalType<typeof NodeTableStore>,
    ) => {
      const nodes = []
      const links = []

      for (const node of action.values()) {
        for (const neighbor of node.linkState?.neighbors ?? []) {
          if (
            neighbor > node.nodeId &&
            action.get(neighbor)?.linkState?.neighbors.includes(node.nodeId)
          ) {
            links.push({
              source: node.nodeId,
              target: neighbor,
            })
          }
        }

        nodes.push(
          previousGraphData.nodes.find(({ id }) => id === node.nodeId) ?? {
            id: node.nodeId,
          },
        )
      }

      return {
        nodes,
        links,
      }
    },
    { nodes: [], links: [] },
  )

  useEffect(() => {
    dispatchGraphData(nodeTable)
  }, [nodeTable])

  if (!basicState || !ilpAllocationScheme || !routingTable) {
    return <div>Loading...</div>
  }

  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-2xl font-bold tracking-tight">Network</h2>
      <NetworkGraph graphData={graphData} className="h-xl" />
      <div className="flex flex-col gap-3 px-3">
        {sortedNodeTable?.map((node) => (
          <div key={node.nodeId} className="">
            <h3 className="text-lg font-bold">
              {node.nodeId}
              {basicState.nodeId === node.nodeId ?
                <span className="bg-slate-6 rounded-full px-3 py-1 ml-2">
                  THIS NODE
                </span>
              : null}
            </h3>
            <p className="text-sm text-muted-foreground">
              {node.linkState?.url ?? "no link state"}
            </p>
          </div>
        ))}
      </div>
    </div>
  )
}
