import useSize from "@react-hook/size"
import type { GraphData } from "force-graph"
import { useCallback, useEffect, useRef, useState } from "react"
import ReactForceGraph2d, {
  ForceGraphMethods,
  NodeObject,
} from "react-force-graph-2d"
import { useLocation } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"

import type { PeerMessageMetadata } from "../../../backend/topics/peer-traffic"
import { COLORS } from "../../constants/palette"
import { activeNodesStore } from "../../remote-signals/active-nodes"
import { peeringStateStore } from "../../remote-signals/peering-state"
import { peerTrafficTopic } from "../../remote-topics/peer-traffic"
import { useSig } from "../../utils/remote-reactive"

interface NodeGraphViewProperties {
  graphData: GraphData
}

const NodeGraphView = ({ graphData }: NodeGraphViewProperties) => {
  const sig = useSig()
  const rootReference = useRef<HTMLDivElement>(null)
  const forceGraphReference = useRef<ForceGraphMethods>()
  const [width, height] = useSize(rootReference)
  const [, setLocation] = useLocation()

  const handleNodeClick = useCallback(
    ({ id }: NodeObject) => setLocation(`/nodes/${String(id)}`),
    [setLocation]
  )

  const generateLinkColor = useCallback(() => "#ffffff", [])
  const generateNodeColor = useCallback(
    ({ id }: NodeObject) => selectBySeed(COLORS, String(id ?? "")),
    []
  )

  sig.on(
    peerTrafficTopic,
    useCallback(
      (data: PeerMessageMetadata | undefined) => {
        if (!data) return

        const link = graphData.links.find(
          ({ source, target }) =>
            typeof source === "object" &&
            data.from === source.id &&
            typeof target === "object" &&
            data.to === target.id
        )
        if (link) forceGraphReference.current?.emitParticle(link)
      },
      [forceGraphReference, graphData]
    )
  )

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
  const sig = useSig()
  const nodes = sig.get(activeNodesStore)
  const peeringState = sig.get(peeringStateStore)
  const [graphData, setGraphData] = useState<GraphData>({
    nodes: [],
    links: [],
  })

  useEffect(() => {
    const links = Object.entries(peeringState ?? {}).flatMap(
      ([nodeId, peers]) =>
        peers.flatMap((peer) => {
          // Strip subnet identifier
          const peerId = peer.split(".")[1]
          return peerId &&
            peerId > nodeId &&
            peeringState?.[peerId]?.includes(`stub.${nodeId}`)
            ? [
                { source: nodeId, target: peerId },
                { source: peerId, target: nodeId },
              ]
            : []
        })
    )

    setGraphData({
      nodes: (nodes || []).map(
        (node) =>
          graphData.nodes.find(({ id }) => id === node.id) ?? { id: node.id }
      ),
      links,
    })
  }, [nodes, peeringState])

  // return <NodeGraphView graphData={graphData} />
  return <NodeGraphView graphData={graphData} />
}

export default NodeGraph
