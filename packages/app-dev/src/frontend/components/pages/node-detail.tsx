import { createTRPCClient } from "@trpc/client"
import { Link, useParams } from "solid-app-router"
import { Component, For, createMemo, createResource } from "solid-js"
import { format } from "timeago.js"

import { selectBySeed } from "@xen-ilp/lib-logger"
import { assertDefined } from "@xen-ilp/lib-type-utils"

import { NODES } from "../../../backend/constants/development-nodes"
import type { DebugRpcRouter } from "../../../runner/effects/debug-rpc-server"
import { COLORS } from "../../constants/palette"
import { logs } from "../../signals/logs"
import LogViewer from "../log-viewer/log-viewer"

const createDebugRPCClient = (nodeId: string) => {
  const port = NODES.find((node) => node.id === nodeId)?.debugPort

  assertDefined(port)

  const client = createTRPCClient<DebugRpcRouter>({
    url: `http://localhost:${port}/trpc`,
  })

  return client
}

interface BasicNodeElementProperties {
  nodeId: string
}
const NodeHeader: Component<BasicNodeElementProperties> = (properties) => {
  const color = createMemo(() => selectBySeed(COLORS, properties.nodeId))
  return (
    <header>
      <div class="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
        <h1 class="font-bold leading-tight text-3xl text-gray-100">
          <i
            class="rounded-full h-5 mr-4 w-5 inline-block"
            style={{ background: color() }}
          ></i>
          Node:{" "}
          <span
            style={{
              color: color(),
            }}
          >
            {properties.nodeId}
          </span>
        </h1>
      </div>
    </header>
  )
}

const NodeStateViewer: Component<BasicNodeElementProperties> = (properties) => {
  const [peerTable] = createResource(
    () => properties.nodeId,
    () => {
      const client = createDebugRPCClient(properties.nodeId)
      return client.query("getPeerTable")
    }
  )

  return (
    <div>
      <h2 class="font-bold text-xl">Peer Table</h2>
      <table class="border-separate border-spacing-2 my-4 -ml-2">
        <thead>
          <tr>
            <th class="text-left">Peer</th>
            <th class="text-left">URL</th>
            <th class="text-left">Last Seen</th>
          </tr>
        </thead>
        <tbody>
          <For each={Object.entries(peerTable() ?? {})}>
            {([, peer]) => (
              <tr>
                <td class="">
                  <Link
                    href={`/nodes/${peer.nodeId}`}
                    class={`font-bold`}
                    style={{ color: selectBySeed(COLORS, peer.nodeId) }}
                  >
                    {peer.nodeId}
                  </Link>
                </td>
                <td class="">
                  <a href={peer.url} rel="external">
                    {peer.url}
                  </a>
                </td>
                <td class="" title={new Date(peer.lastSeen).toISOString()}>
                  {format(new Date(peer.lastSeen))}
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  )
}

const NodeLogViewer: Component<BasicNodeElementProperties> = (properties) => {
  const nodeLogs = createMemo(() =>
    logs().filter(({ node }) => node === properties.nodeId)
  )
  return <LogViewer logs={nodeLogs()} />
}

const NodeDetail: Component = () => {
  const parameters = useParams()
  assertDefined(parameters["nodeId"])

  return (
    <div class="flex flex-col h-full max-h-screen min-h-0 py-10">
      <NodeHeader nodeId={parameters["nodeId"]} />
      <main class="flex flex-col mx-auto flex-1 min-h-0 w-full max-w-7xl pt-8 sm:px-6 lg:px-8">
        <NodeStateViewer nodeId={parameters["nodeId"]} />
        <NodeLogViewer nodeId={parameters["nodeId"]} />
      </main>
    </div>
  )
}

export default NodeDetail
