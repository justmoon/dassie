import type { GraphData } from "force-graph"
import type { Component } from "solid-js"

import type { InputConfig } from "@xen-ilp/app-node"

import { NODES } from "../../../backend/constants/development-nodes"
import type { NodeDefinition } from "../../../backend/effects/run-nodes"
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
    <div class="flex flex-col flex-1 py-10">
      <header>
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 class="font-bold leading-tight text-3xl text-gray-100">
            Dashboard
          </h1>
        </div>
      </header>
      <main class="flex flex-col mx-auto flex-1 min-h-0 max-w-7xl sm:px-6 lg:px-8">
        <div class="h-md py-8 px-4 sm:px-0">
          <NodeGraph graphData={nodesToGraph(NODES)} />
        </div>
        <LogViewer />
      </main>
    </div>
  )
}

export default Dashboard
