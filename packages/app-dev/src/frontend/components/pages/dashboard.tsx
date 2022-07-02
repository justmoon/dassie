import type { GraphData } from "force-graph"
import type { Component } from "solid-js"

import type { InputConfig } from "@xen-ilp/app-node"

import { NODES } from "../../../constants/development-nodes"
import type { NodeDefinition } from "../../../servers/node-server"
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
const Dashboard: Component = () => {
  return (
    <div class="flex-1 py-10">
      <header>
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 class="font-bold leading-tight text-3xl text-gray-100">
            Dashboard
          </h1>
        </div>
      </header>
      <main>
        <div class="flex-col mx-auto max-w-7xl sm:px-6 lg:px-8">
          <div class="h-md py-8 px-4 sm:px-0">
            <NodeGraph graphData={nodesToGraph(NODES)} />
          </div>
          <div class="flex-1 py-8 px-4 sm:px-0">
            <LogViewer />
          </div>
        </div>
      </main>
    </div>
  )
}

export default Dashboard
