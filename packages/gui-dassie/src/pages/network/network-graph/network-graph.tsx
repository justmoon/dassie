import useSize from "@react-hook/size"
import {
  type ComponentPropsWithoutRef,
  useCallback,
  useEffect,
  useRef,
} from "react"
import ReactForceGraph2d, {
  type ForceGraphMethods,
  type GraphData,
  type NodeObject,
} from "react-force-graph-2d"

import { selectBySeed } from "@dassie/lib-logger"
import { type ReadonlyTopic, createScope } from "@dassie/lib-reactive"

import { COLORS } from "../../../constants/palette"
import { combine } from "../../../utils/class-helper"

export interface PeerTrafficEvent {
  from: string
  to: string
}

type PeerTrafficTopic = ReadonlyTopic<PeerTrafficEvent>

interface NetworkGraphProperties extends ComponentPropsWithoutRef<"div"> {
  graphData: GraphData
  peerTraffic?: PeerTrafficTopic | undefined
  onNodeClick?: ((nodeId: string) => void) | undefined
}

export type { GraphData } from "react-force-graph-2d"

export const NetworkGraph = ({
  graphData,
  peerTraffic,
  onNodeClick,
  className,
  ...remainingProperties
}: NetworkGraphProperties) => {
  const rootReference = useRef<HTMLDivElement>(null)
  const forceGraphReference = useRef<ForceGraphMethods>()
  const [width, height] = useSize(rootReference)

  const handleNodeClick = useCallback(
    ({ id }: NodeObject) => {
      if (id && typeof id === "string") onNodeClick?.(id)
    },
    [onNodeClick],
  )

  const generateLinkColor = useCallback(() => "#ffffff", [])
  const generateNodeColor = useCallback(
    ({ id }: NodeObject) => selectBySeed(COLORS, String(id ?? "")),
    [],
  )

  useEffect(() => {
    if (!peerTraffic) return

    const scope = createScope("peer-traffic-listener")

    peerTraffic.on(scope, ({ from, to }) => {
      const link = graphData.links.find(
        ({ source, target }) =>
          typeof source === "object" &&
          from === source.id &&
          typeof target === "object" &&
          to === target.id,
      )
      if (link) forceGraphReference.current?.emitParticle(link)
    })
  }, [peerTraffic, forceGraphReference, graphData])

  return (
    <div
      className={combine("h-full relative", className)}
      {...remainingProperties}
      ref={rootReference}
    >
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
