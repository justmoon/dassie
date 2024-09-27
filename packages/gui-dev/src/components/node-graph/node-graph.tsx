import { useCallback, useEffect, useReducer, useState } from "react"

import type { PeerMessageMetadata } from "@dassie/app-dev/src/topics/peer-traffic"
import {
  type GraphData,
  NetworkGraph,
  type PeerTrafficEvent,
} from "@dassie/gui-dassie/src/pages/network/network-graph/network-graph"
import { createTopic } from "@dassie/lib-reactive"
import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import { rpc } from "../../utils/rpc"

const generateNode = (nodeId: string) => ({ id: nodeId })

const NodeGraph = () => {
  const peeringState = useRemoteSignal(rpc.subscribeToPeeringState)
  const [peerTrafficTopic] = useState(() => createTopic<PeerTrafficEvent>())

  rpc.subscribeToPeerTraffic.useSubscription(undefined, {
    onData: useCallback(
      (data: PeerMessageMetadata | undefined) => {
        if (!data) return

        peerTrafficTopic.emit(data)
      },
      [peerTrafficTopic],
    ),
  })

  const [graphData, dispatchGraphData] = useReducer(
    (
      previousGraphData: GraphData,
      action: { nodes: string[]; peeringState: typeof peeringState },
    ) => {
      const links = Object.entries(action.peeringState ?? {}).flatMap(
        ([nodeId, peers]) =>
          peers.flatMap((peerId) => {
            return (
                peerId &&
                  peerId > nodeId &&
                  action.peeringState?.[peerId]?.includes(nodeId)
              ) ?
                [
                  { source: nodeId, target: peerId },
                  { source: peerId, target: nodeId },
                ]
              : []
          }),
      )

      return {
        nodes: action.nodes.map(
          (nodeId) =>
            previousGraphData.nodes.find(({ id }) => id === nodeId) ??
            generateNode(nodeId),
        ),
        links,
      }
    },
    {
      nodes: [],
      links: [],
    },
  )

  useEffect(() => {
    if (!peeringState) return

    dispatchGraphData({ nodes: Object.keys(peeringState), peeringState })
  }, [peeringState])

  return <NetworkGraph graphData={graphData} peerTraffic={peerTrafficTopic} />
}

export default NodeGraph
