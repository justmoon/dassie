import type { ViteDevServer } from "vite"
import type { ViteNodeServer } from "vite-node/server"

import type { EffectContext } from "@dassie/lib-reactive"

import { runChildProcess } from "./run-child-process"
import type { NodeDefinition } from "./run-nodes"

interface RunNodeChildProcessProperties<T> {
  viteServer: ViteDevServer
  nodeServer: ViteNodeServer
  node: NodeDefinition<T>
}

export const runNodeChildProcess = async <T>(
  sig: EffectContext,
  { viteServer, nodeServer, node }: RunNodeChildProcessProperties<T>
) => {
  await sig.run(runChildProcess, {
    nodeServer,
    id: node.id,
    environment: {
      FORCE_COLOR: "1",
      ...process.env,
      DASSIE_LOG_FORMATTER: "json",
      DASSIE_CONFIG: JSON.stringify(node.config),
      DASSIE_DEV_ROOT: viteServer.config.root,
      DASSIE_DEV_BASE: viteServer.config.base,
      DASSIE_DEV_ENTRY: node.entry ?? "src/index.ts",
      DASSIE_DEV_RPC_URL: "wss://dev-rpc.localhost:10001",
      DASSIE_DEV_NODE_ID: node.id,
      DASSIE_DEBUG_RPC_PORT: String(node.debugPort),
    },
  })
}
