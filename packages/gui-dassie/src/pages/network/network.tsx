import { useCallback, useEffect, useReducer } from "react"
import { useLocation } from "wouter"

import { NodeTableStore } from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import type { NodeId } from "@dassie/app-dassie/src/peer-protocol/types/node-id"
import type { InferSignalType } from "@dassie/lib-reactive"
import {
  useRemoteSignal,
  useRemoteStore,
} from "@dassie/lib-reactive-rpc/client"

import { rpc } from "../../utils/rpc"
import { type GraphData, NetworkGraph } from "./network-graph/network-graph"
import { NodeDetail } from "./node-detail/node-detail"
import { OwnNodeDetail } from "./node-detail/own-node-detail"
import { NodeListing } from "./node-listing/node-listing"

interface NetworkPageProperties {
  params: {
    nodeId?: string | undefined
  }
}

export function NetworkPage({ params: { nodeId } }: NetworkPageProperties) {
  const [, setLocation] = useLocation()

  const nodeTable = useRemoteStore(
    rpc.network.subscribeToNodeTableStore,
    NodeTableStore,
  )
  const routingTable = useRemoteSignal(rpc.debug.subscribeRoutingTable)

  const { data: basicState } = rpc.general.getBasicState.useQuery()

  const handleNodeClick = useCallback(
    (nodeId: string) => {
      setLocation(`/network/${String(nodeId)}`)
    },
    [setLocation],
  )

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

  if (!basicState || !routingTable || basicState.state !== "authenticated") {
    return <div>Loading...</div>
  }

  const selectedNodeId = nodeId as NodeId
  const selectedNodeEntry = nodeTable.get(selectedNodeId)
  const selectedNodeRoute = routingTable.get(
    `${basicState.ilpAllocationScheme}.das.${selectedNodeId}`,
  )

  return (
    <div className="grid grid-cols-1 grid-rows-[auto_1fr_2fr] gap-4 md:grid-cols-2">
      <h2 className="text-2xl font-bold tracking-tight md:col-span-2">
        Network {nodeId}
      </h2>
      <NetworkGraph
        graphData={graphData}
        className="h-xl md:col-span-2"
        onNodeClick={handleNodeClick}
      />
      <NodeListing nodeTable={nodeTable} routingTable={routingTable} />
      {!nodeId || nodeId === basicState.nodeId ?
        <OwnNodeDetail className="hidden md:flex" />
      : <NodeDetail
          nodeId={selectedNodeId}
          nodeTableEntry={selectedNodeEntry}
          nodeRoute={selectedNodeRoute}
        />
      }
    </div>
  )
}
