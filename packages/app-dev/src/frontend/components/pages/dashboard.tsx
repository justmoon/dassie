import type { GraphData } from "force-graph"
import { withSolid } from "react-solid-state"
import { Link } from "wouter"

import type { InputConfig } from "@xen-ilp/app-node"
import { selectBySeed } from "@xen-ilp/lib-logger"

import type { NodeDefinition } from "../../../backend/effects/run-nodes"
import { COLORS } from "../../constants/palette"
import { activeTemplate } from "../../signals/active-template"
import LogViewer from "../log-viewer/log-viewer"
import NodeGraph from "../node-graph/node-graph"

const nodesToGraph = (nodes: NodeDefinition<InputConfig>[]): GraphData => {
  return {
    nodes: nodes.map((node) => ({ id: node.id })),
    links: nodes.flatMap((node) =>
      node.peers.map((peer) => ({ source: node.id, target: peer }))
    ),
  }
}
const Dashboard = withSolid(() => () => {
  const nodes = activeTemplate()

  if (!nodes) return <div />

  return (
    <div className="h-screen grid grid-rows-[min-content_auto] py-10">
      <header>
        <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 className="font-bold leading-tight text-3xl text-gray-100">
            Dashboard
          </h1>
        </div>
      </header>
      <main className="min-h-0 relative">
        <div className="h-full mx-auto min-h-0 max-w-7xl grid grid-rows-[min-content_auto] py-8 px-4 gap-4 grid-cols-[300px_auto] sm:px-6 sm:px-0 lg:px-8">
          <div className="rounded-lg bg-gray-800 min-h-0 p-4">
            <h2 className="font-bold text-xl">Nodes</h2>
            <div className="flex flex-col py-2 gap-1">
              {nodes.map((node) => (
                <Link
                  key={node.id}
                  href={`/nodes/${node.id}`}
                  className="block"
                >
                  <i
                    className="rounded-full h-2 mr-2 w-2 inline-block"
                    style={{ background: selectBySeed(COLORS, node.id) }}
                  ></i>
                  {node.id}
                </Link>
              ))}
            </div>
          </div>
          <div className="h-lg">
            <NodeGraph graphData={nodesToGraph(nodes)} />
          </div>
          <div className="rounded-lg bg-gray-800 min-h-0 p-4 col-span-2">
            <LogViewer />
          </div>
        </div>
      </main>
    </div>
  )
})

export default Dashboard
