import { useCallback, useEffect, useMemo, useReducer } from "react"
import type { LinkObject, NodeObject } from "react-force-graph-2d"
import { useLocation } from "wouter"

import { NodeTableStore } from "@dassie/app-dassie/src/peer-protocol/stores/node-table"
import type { NodeId } from "@dassie/app-dassie/src/peer-protocol/types/node-id"
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

  const graphData = useMemo(() => {
    const nodes: NodeObject[] = []
    const links: LinkObject[] = []

    if (basicState?.state !== "authenticated") return { nodes, links }

    for (const node of nodeTable.values()) {
      nodes.push({ id: node.nodeId })

      if (node.peerState.id === "peered") {
        links.push({
          source: basicState.nodeId,
          target: node.nodeId,
        })
      }

      if (node.nodeId !== basicState.nodeId) {
        for (const neighbor of node.linkState?.neighbors ?? []) {
          if (
            neighbor !== basicState.nodeId &&
            neighbor > node.nodeId &&
            nodeTable.get(neighbor)?.linkState?.neighbors.includes(node.nodeId)
          ) {
            links.push({
              source: node.nodeId,
              target: neighbor,
            })
          }
        }
      }
    }

    return { nodes, links }
  }, [nodeTable, basicState])

  // We need to make sure that existing nodes' positions are preserved when the
  // graph is updated.
  const [stableGraphData, dispatchGraphData] = useReducer(
    (previousGraphData: GraphData, { nodes, links }: GraphData) => {
      return {
        nodes: nodes.map(
          (node) =>
            previousGraphData.nodes.find((oldNode) => oldNode.id === node.id) ??
            node,
        ),
        links,
      }
    },
    { nodes: [], links: [] },
  )

  useEffect(() => {
    dispatchGraphData(graphData)
  }, [graphData])

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
        graphData={stableGraphData}
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
