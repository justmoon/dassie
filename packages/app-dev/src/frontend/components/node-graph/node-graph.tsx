import ForceGraph, { ForceGraphInstance, GraphData } from "force-graph"
import { useNavigate } from "solid-app-router"
import { Component, createEffect, onCleanup, onMount } from "solid-js"

import { selectBySeed } from "@xen-ilp/lib-logger"

import { COLORS } from "../../constants/palette"

interface NodeGraphProperties {
  graphData: GraphData
}

const NodeGraph: Component<NodeGraphProperties> = (properties) => {
  let graphReference!: HTMLDivElement
  let graph: ForceGraphInstance | undefined
  const navigate = useNavigate()

  onMount(() => {
    graph = ForceGraph()(graphReference)
      .linkDirectionalParticles(1)
      .linkColor(() => "#ffffff")
      .nodeColor(({ id }) => selectBySeed(COLORS, String(id ?? "")))
      .nodeLabel("id")
      .onNodeClick(({ id }) => navigate(`/nodes/${String(id)}`))
      .graphData(properties.graphData)

    const resizeObserver = new ResizeObserver(([entry]) => {
      if (!graph) return
      const size = entry?.contentBoxSize[0]
      if (size) {
        graph.width(size.inlineSize)
        graph.height(size.blockSize)
      }
    })

    resizeObserver.observe(graphReference)

    onCleanup(() => graph?._destructor())
  })

  createEffect(() => {
    graph?.graphData(properties.graphData)
  })

  return <div class="h-md" ref={graphReference} />
}

export default NodeGraph
