import type { ViteDevServer } from "vite"
import type { ViteNodeServer } from "vite-node/server"

import { createActor } from "@dassie/lib-reactive"

import { DEBUG_RPC_PORT } from "../constants/ports"
import { debugScopesSignal } from "../signals/debug-scopes"
import { runChildProcess } from "./run-child-process"
import type { NodeDefinition } from "./run-nodes"

interface RunNodeChildProcessProperties<T> {
  viteServer: ViteDevServer
  nodeServer: ViteNodeServer
  node: NodeDefinition<T>
}

export const runNodeChildProcess = <T>() =>
  createActor(
    async (
      sig,
      { viteServer, nodeServer, node }: RunNodeChildProcessProperties<T>
    ) => {
      const debugScopes = sig.get(debugScopesSignal)
      await sig.run(runChildProcess, {
        nodeServer,
        id: node.id,
        environment: {
          FORCE_COLOR: "1",
          ...process.env,
          DEBUG: debugScopes,
          DEBUG_HIDE_DATE: "1",
          DASSIE_LOG_FORMATTER: "json",
          DASSIE_CONFIG: JSON.stringify(node.config),
          DASSIE_DEV_ROOT: viteServer.config.root,
          DASSIE_DEV_BASE: viteServer.config.base,
          DASSIE_DEV_ENTRY: node.entry ?? "src/index.ts",
          DASSIE_DEV_RPC_URL: `wss://dev-rpc.localhost:${DEBUG_RPC_PORT}`,
          DASSIE_DEV_NODE_ID: node.id,
          DASSIE_DEBUG_RPC_PORT: String(node.debugPort),
        },
      })
    }
  )
