import useSize from "@react-hook/size"
import type { GraphData } from "force-graph"
import { useCallback, useRef } from "react"
import ReactForceGraph2d, { ForceGraphMethods } from "react-force-graph-2d"
import { useLocation } from "wouter"

import { selectBySeed } from "@dassie/lib-logger"
import { useSig } from "@dassie/lib-reactive-trpc/client"

import type { PeerMessageMetadata } from "../../../backend/topics/peer-traffic"
import { COLORS } from "../../constants/palette"
import { peerTrafficTopic } from "../../remote-topics/peer-traffic"

interface NodeGraphProperties {
  graphData: GraphData
}

const NodeGraph = ({ graphData }: NodeGraphProperties) => {
  const rootReference = useRef<HTMLDivElement>(null)
  const forceGraphReference = useRef<ForceGraphMethods>()
  const [width, height] = useSize(rootReference)
  const [, setLocation] = useLocation()
  const sig = useSig()

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
    <div className="h-full" ref={rootReference}>
      <ReactForceGraph2d
        ref={forceGraphReference}
        graphData={graphData}
        width={width}
        height={height}
        linkColor={() => "#ffffff"}
        nodeColor={({ id }) => selectBySeed(COLORS, String(id ?? ""))}
        nodeLabel={"id"}
        linkDirectionalParticleSpeed={0.08}
        onNodeClick={({ id }) => setLocation(`/nodes/${String(id)}`)}
      />
    </div>
  )
}

export default NodeGraph
