import useSize from "@react-hook/size"
import type { GraphData } from "force-graph"
import { useRef } from "react"
import ReactForceGraph2d from "react-force-graph-2d"
import { useLocation } from "wouter"

import { selectBySeed } from "@xen-ilp/lib-logger"

import { COLORS } from "../../constants/palette"

interface NodeGraphProperties {
  graphData: GraphData
}

const NodeGraph = ({ graphData }: NodeGraphProperties) => {
  const rootReference = useRef<HTMLDivElement>(null)
  const [width, height] = useSize(rootReference)
  const [, setLocation] = useLocation()
  // .linkDirectionalParticles(1)
  // .linkColor(() => "#ffffff")
  // .nodeColor(({ id }) => selectBySeed(COLORS, String(id ?? "")))
  // .nodeLabel("id")
  // .graphData(properties.graphData)
  return (
    <div className="h-full" ref={rootReference}>
      <ReactForceGraph2d
        graphData={graphData}
        width={width}
        height={height}
        linkColor={() => "#ffffff"}
        nodeColor={({ id }) => selectBySeed(COLORS, String(id ?? ""))}
        nodeLabel={"id"}
        onNodeClick={({ id }) => setLocation(`/nodes/${String(id)}`)}
      />
    </div>
  )
}

export default NodeGraph
