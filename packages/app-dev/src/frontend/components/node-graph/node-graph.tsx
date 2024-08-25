import useSize from "@react-hook/size"
import { useCallback, useEffect, useReducer, useRef } from "react"
import ReactForceGraph2d, {
  type ForceGraphMethods,
  type GraphData,
  type NodeObject,
} from "react-force-graph-2d"
import { useLocation } from "wouter"

import { COLORS } from "@dassie/app-node/src/frontend/constants/palette"
import { selectBySeed } from "@dassie/lib-logger"
import { useRemoteSignal } from "@dassie/lib-reactive-rpc/client"

import type { PeerMessageMetadata } from "../../../backend/topics/peer-traffic"
import { rpc } from "../../utils/rpc"

interface NodeGraphViewProperties {
  graphData: GraphData
}

const generateNode = (nodeId: string) => ({ id: nodeId })

const NodeGraphView = ({ graphData }: NodeGraphViewProperties) => {
  const rootReference = useRef<HTMLDivElement>(null)
  const forceGraphReference = useRef<ForceGraphMethods>()
  const [width, height] = useSize(rootReference)
  const [, setLocation] = useLocation()

  const handleNodeClick = useCallback(
    ({ id }: NodeObject) => {
      setLocation(`/nodes/${String(id)}`)
    },
    [setLocation],
  )

  const generateLinkColor = useCallback(() => "#ffffff", [])
  const generateNodeColor = useCallback(
    ({ id }: NodeObject) => selectBySeed(COLORS, String(id ?? "")),
    [],
  )

  rpc.subscribeToPeerTraffic.useSubscription(undefined, {
    onData: useCallback(
      (data: PeerMessageMetadata | undefined) => {
        if (!data) return

        const link = graphData.links.find(
          ({ source, target }) =>
            typeof source === "object" &&
            data.from === source.id &&
            typeof target === "object" &&
            data.to === target.id,
        )
        if (link) forceGraphReference.current?.emitParticle(link)
      },
      [forceGraphReference, graphData],
    ),
  })

  return (
    <div className="h-full relative" ref={rootReference}>
      <div className="absolute inset-0">
        <ReactForceGraph2d
          ref={forceGraphReference}
          graphData={graphData}
          width={width}
          height={height}
          linkColor={generateLinkColor}
          nodeColor={generateNodeColor}
          nodeLabel={"id"}
          linkDirectionalParticleSpeed={0.08}
          onNodeClick={handleNodeClick}
        />
      </div>
    </div>
  )
}

const NodeGraph = () => {
  const peeringState = useRemoteSignal(rpc.subscribeToPeeringState)
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

  return <NodeGraphView graphData={graphData} />
}

export default NodeGraph
