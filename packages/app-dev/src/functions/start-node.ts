import type { AbortContext, Reactor, ScopeContext } from "@dassie/lib-reactive"

import { DEBUG_UI_RPC_PORT } from "../constants/ports"
import { DebugScopesSignal } from "../signals/debug-scopes"
import { SecurityTokenSignal } from "../signals/security-token"
import { ActiveNodesStore } from "../stores/active-nodes"
import type { RunnerEnvironment } from "../types/runner-environment"
import { ViteNodeServer } from "../unconstructables/vite-node-server"
import { ViteServer } from "../unconstructables/vite-server"
import type { NodeConfig } from "../utils/generate-node-config"
import { prefillDatabase } from "../utils/prefill-database"
import { prepareDataDirectory } from "../utils/prepare-data-directory"
import {
  type CertificateInfo,
  validateCertificates,
} from "../utils/validate-certificates"
import { RunChildProcess } from "./run-child-process"

export interface StartNodeParameters {
  node: NodeConfig
  context: ScopeContext & AbortContext
}

export const StartNode = (reactor: Reactor) => {
  const debugScopesSignal = reactor.use(DebugScopesSignal)
  const runChildProcess = reactor.use(RunChildProcess)
  const securityTokenSignal = reactor.use(SecurityTokenSignal)
  const viteServer = reactor.use(ViteServer)
  const viteNodeServer = reactor.use(ViteNodeServer)
  const activeNodesStore = reactor.use(ActiveNodesStore)

  return async ({ node, context }: StartNodeParameters) => {
    if (context.scope.isDisposed || context.cancellable.isCancelled) {
      return
    }

    // Generate TLS certificates
    {
      const neededCertificates: CertificateInfo[] = [
        {
          commonName: `${node.id}.localhost`,
          certificatePath: node.tlsWebCertFile,
          keyPath: node.tlsWebKeyFile,
        },
      ]

      await validateCertificates({
        id: node.id,
        certificates: neededCertificates,
      })
    }

    // Prepare data directory with database
    {
      const { dataPath } = node

      await prepareDataDirectory(dataPath)
      await prefillDatabase(node)
    }

    const debugScopes = debugScopesSignal.read()

    activeNodesStore.act.addNode(node)
    context.scope.onCleanup(() => {
      activeNodesStore.act.removeNode(node)
    })

    await runChildProcess({
      scope: context.scope,
      nodeServer: viteNodeServer,
      id: node.id,
      environment: {
        FORCE_COLOR: "1",
        ...process.env,
        DEBUG: debugScopes,
        DEBUG_HIDE_DATE: "1",
        DASSIE_BOOTSTRAP_NODES: JSON.stringify(node.bootstrapNodes),
        DASSIE_STATE_DIRECTORY: node.dataPath,
        DASSIE_IPC_SOCKET_PATH: node.ipcSocketPath,
        DASSIE_LOG_LEVEL: "none",
        DASSIE_DEV_ROOT: viteServer.config.root,
        DASSIE_DEV_BASE: viteServer.config.base,
        DASSIE_DEV_ENTRY: node.entry,
        DASSIE_DEV_RPC_URL: `wss://dev-rpc.localhost:${DEBUG_UI_RPC_PORT}`,
        DASSIE_DEV_NODE_ID: node.id,
        DASSIE_DEV_SECURITY_TOKEN: securityTokenSignal.read(),
      } satisfies RunnerEnvironment,
      extraArguments: [
        "--stack-trace-limit=64",
        "--trace-deprecation",
        `--inspect-port=${node.debugPort}`,
      ],
    })
  }
}
