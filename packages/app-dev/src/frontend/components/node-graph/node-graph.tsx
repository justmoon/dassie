import useSize from "@react-hook/size"
import type { GraphData } from "force-graph"
import { useCallback, useEffect, useReducer, useRef } from "react"
import ReactForceGraph2d, {
  ForceGraphMethods,
  NodeObject,
} from "react-force-graph-2d"
import { useLocation } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"

import type { PeerMessageMetadata } from "../../../backend/topics/peer-traffic"
import type { NodeConfig } from "../../../backend/utils/generate-node-config"
import { COLORS } from "../../constants/palette"
import { activeNodesStore } from "../../remote-signals/active-nodes"
import { peeringStateStore } from "../../remote-signals/peering-state"
import { peerTrafficTopic } from "../../remote-topics/peer-traffic"
import { useSig } from "../../utils/remote-reactive"

interface NodeGraphViewProperties {
  graphData: GraphData
}

const SHOW_IN_PHYSICAL_LOCATION = false as boolean

const normalizedMercatorProjection = (latitude: number, longitude: number) => {
  const pi = Math.PI

  // Calculate normalized x and y coordinates using Mercator projection
  const x = longitude / (2 * pi) + 0.5
  const y = (Math.log(Math.tan(pi / 4 + latitude / 2)) / pi) * -0.5 + 0.5

  return { x: x, y: y }
}

const generateNode = (node: NodeConfig) => {
  const { x, y } = normalizedMercatorProjection(node.latitude, node.longitude)

  return SHOW_IN_PHYSICAL_LOCATION
    ? {
        id: node.id,
        fx: x * 500 - 250,
        fy: y * 250 - 125,
      }
    : { id: node.id }
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
  const [graphData, dispatchGraphData] = useReducer(
    (
      previousGraphData: GraphData,
      action: { nodes: typeof nodes; peeringState: typeof peeringState }
    ) => {
      const links = Object.entries(action.peeringState ?? {}).flatMap(
        ([nodeId, peers]) =>
          peers.flatMap((peer) => {
            // Strip subnet identifier
            const peerId = peer.split(".")[1]
            return peerId &&
              peerId > nodeId &&
              action.peeringState?.[peerId]?.find((peerId) =>
                peerId.endsWith(`.${nodeId}`)
              )
              ? [
                  { source: nodeId, target: peerId },
                  { source: peerId, target: nodeId },
                ]
              : []
          })
      )

      return {
        nodes: (action.nodes ?? []).map(
          (node) =>
            previousGraphData.nodes.find(({ id }) => id === node.id) ??
            generateNode(node)
        ),
        links,
      }
    },
    {
      nodes: [],
      links: [],
    }
  )

  useEffect(() => {
    dispatchGraphData({ nodes, peeringState })
  }, [nodes, peeringState])

  // return <NodeGraphView graphData={graphData} />
  return <NodeGraphView graphData={graphData} />
}

export default NodeGraph
