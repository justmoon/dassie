import type { GraphData } from "force-graph"
import type { Component } from "solid-js"

import type { InputConfig } from "@xen-ilp/app-node"

import { NODES } from "../../../backend/constants/development-nodes"
import type { NodeDefinition } from "../../../backend/effects/run-nodes"
import { logs } from "../../signals/logs"
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
    <div class="flex flex-col h-screen max-h-screen min-h-0 py-10 items-stretch">
      <header>
        <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
          <h1 class="font-bold leading-tight text-3xl text-gray-100">
            Dashboard
          </h1>
        </div>
      </header>
      <main class="flex-1 min-h-0 relative">
        <div class="flex flex-col h-full mx-auto min-h-0 max-w-7xl py-8 px-4 sm:px-6 sm:px-0 lg:px-8">
          <NodeGraph graphData={nodesToGraph(NODES)} />
          <LogViewer logs={logs()} />
        </div>
      </main>
    </div>
  )
}

export default Dashboard
