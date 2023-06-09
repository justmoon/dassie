import { createLogger } from "@dassie/lib-logger"
import { createActor, createMapped } from "@dassie/lib-reactive"

import { DEBUG_RPC_PORT } from "../constants/ports"
import { viteNodeService } from "../services/vite-node-server"
import { viteService } from "../services/vite-server"
import { debugScopesSignal } from "../signals/debug-scopes"
import { activeNodesStore } from "../stores/active-nodes"
import { environmentSettingsStore } from "../stores/environment-settings"
import { generateNodeConfig } from "../utils/generate-node-config"
import { prefillDatabase } from "../utils/prefill-database"
import { prepareDataDirectory } from "../utils/prepare-data-directory"
import {
  type CertificateInfo,
  validateCertificates,
} from "../utils/validate-certificates"
import { fileChangeTopic } from "./handle-file-change"
import { runChildProcess } from "./run-child-process"

const logger = createLogger("das:dev:run-nodes")

export interface NodeDefinition<T> {
  id: string
  port: number
  debugPort: number
  peers: string[]
  config: T
  url: string
  entry?: string
}

export const runNodes = () =>
  createActor(async (sig) => {
    const viteServer = sig.get(viteService)
    const nodeServer = sig.get(viteNodeService)

    if (!viteServer || !nodeServer) return

    // Restart all nodes when a source code file changes
    sig.subscribe(fileChangeTopic)

    logger.debug("starting node processes")

    const nodeActorMap = () =>
      createMapped(activeNodesStore, (nodeId) =>
        createActor(async (sig) => {
          const environmentSettings = sig.get(environmentSettingsStore)
          const node = generateNodeConfig(nodeId, environmentSettings)

          // Generate TLS certificates
          {
            const neededCertificates: CertificateInfo[] = [
              {
                type: "web",
                commonName: `${node.id}.localhost`,
                certificatePath: node.config.tlsWebCertFile,
                keyPath: node.config.tlsWebKeyFile,
              },
              {
                type: "dassie",
                commonName: `test.das.${node.id}`,
                certificatePath: node.config.tlsDassieCertFile,
                keyPath: node.config.tlsDassieKeyFile,
              },
            ]

            await validateCertificates({
              id: node.id,
              certificates: neededCertificates,
            })
          }

          // Prepare data directory with database
          {
            const { dataPath } = node.config

            await prepareDataDirectory(dataPath)
            await prefillDatabase({ dataPath })
          }

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
              DASSIE_DEV_ENTRY: node.entry,
              DASSIE_DEV_RPC_URL: `wss://dev-rpc.localhost:${DEBUG_RPC_PORT}`,
              DASSIE_DEV_NODE_ID: node.id,
              DASSIE_DEBUG_RPC_PORT: String(node.debugPort),
            },
          })
        })
      )

    await Promise.all(sig.runMap(nodeActorMap))
  })
