import { selectBySeed } from "@dassie/lib-logger"
import useSize from "@react-hook/size"
import type { GraphData } from "force-graph"
import { useRef } from "react"
import ReactForceGraph2d, { ForceGraphMethods } from "react-force-graph-2d"
import { useLocation } from "wouter"

import { COLORS } from "../../constants/palette"
import { trpc } from "../../utils/trpc"

interface NodeGraphProperties {
  graphData: GraphData
}

const NodeGraph = ({ graphData }: NodeGraphProperties) => {
  const rootReference = useRef<HTMLDivElement>(null)
  const forceGraphReference = useRef<ForceGraphMethods>()
  const [width, height] = useSize(rootReference)
  const [, setLocation] = useLocation()

  trpc.useSubscription(["peerTraffic"], {
    onNext: (data) => {
      const link = graphData.links.find(
        ({ source, target }) =>
          typeof source === "object" &&
          data.from === source.id &&
          typeof target === "object" &&
          data.to === target.id
      )
      if (link) forceGraphReference.current?.emitParticle(link)
    },
  })

  // .linkDirectionalParticles(1)
  // .linkColor(() => "#ffffff")
  // .nodeColor(({ id }) => selectBySeed(COLORS, String(id ?? "")))
  // .nodeLabel("id")
  // .graphData(properties.graphData)
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
