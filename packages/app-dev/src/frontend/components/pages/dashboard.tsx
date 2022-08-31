import type { GraphData } from "force-graph"
import { FaPlus } from "react-icons/fa"
import { Link } from "wouter"

import type { InputConfig } from "@dassie/app-node"
import { selectBySeed } from "@dassie/lib-logger"
import { useSig } from "@dassie/lib-reactive-react"

import type { NodeDefinition } from "../../../backend/effects/run-nodes"
import { generateNodeConfig } from "../../../backend/utils/generate-node-config"
import { COLORS } from "../../constants/palette"
import { activeTemplate } from "../../remote-stores/active-template"
import { trpc } from "../../utils/trpc"
import LogViewer from "../log-viewer/log-viewer"
import NodeGraph from "../node-graph/node-graph"

const nodesToGraph = (nodes: NodeDefinition<InputConfig>[]): GraphData => {
  return {
    nodes: nodes.map((node) => ({ id: node.id })),
    links: nodes.flatMap((node) =>
      node.peers.flatMap((peer) => [
        { source: node.id, target: peer },
        { source: peer, target: node.id },
      ])
    ),
  }
}
const Dashboard = () => {
  const sig = useSig()
  const template = sig.get(activeTemplate)
  const addRandomNode = trpc.useMutation("addRandomNode")

  if (!template) return <div />

  const nodes = template.map((peers, index) => generateNodeConfig(index, peers))

  return (
    <div className="h-screen grid grid-rows-[min-content_auto] py-10 gap-4">
      <header>
        <div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 className="font-bold leading-tight text-3xl text-gray-100">
            Dashboard
          </h1>
        </div>
      </header>
      <main className="min-h-0 relative">
        <div className="h-full mx-auto min-h-0 max-w-7xl grid grid-rows-[min-content_auto] px-4 gap-4 grid-cols-[300px_auto] sm:px-6 sm:px-0 lg:px-8">
          <div className="rounded-lg flex flex-col bg-gray-800 min-h-0 p-4">
            <h2 className="font-bold text-xl">Nodes</h2>
            <div className="flex flex-col flex-1 py-2 gap-1">
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
            <div>
              <button
                type="button"
                className="rounded-lg font-medium bg-blue-700 text-white text-sm text-center mr-2 p-2.5 inline-flex items-center dark:bg-blue-600 hover:bg-blue-800 focus:outline-none focus:ring-4 focus:ring-blue-300 dark:hover:bg-blue-700 dark:focus:ring-blue-800"
                onClick={() => {
                  addRandomNode.mutate()
                }}
              >
                <FaPlus className="flex-shrink-0 text-lg" />
                <span className="sr-only">Add node</span>
              </button>
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
}

export default Dashboard
