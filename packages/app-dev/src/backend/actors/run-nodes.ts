import { setTimeout } from "node:timers/promises"

import { createActor, createMapped } from "@dassie/lib-reactive"

import { RunnerEnvironment } from "../../common/types/runner-environment"
import { DEBUG_RPC_PORT } from "../constants/ports"
import { children as logger } from "../logger/instances"
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

// Amount of time to wait between starting each node process
const NODE_STARTUP_INTERVAL = 500

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
                certificatePath: node.tlsWebCertFile,
                keyPath: node.tlsWebKeyFile,
              },
              {
                type: "dassie",
                commonName: `test.das.${node.id}`,
                certificatePath: node.dassieCertFile,
                keyPath: node.dassieKeyFile,
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

          const debugScopes = sig.get(debugScopesSignal)
          await sig.run(runChildProcess, {
            nodeServer,
            id: node.id,
            environment: {
              FORCE_COLOR: "1",
              ...process.env,
              DEBUG: debugScopes,
              DEBUG_HIDE_DATE: "1",
              DASSIE_BOOTSTRAP_NODES: JSON.stringify(node.bootstrapNodes),
              DASSIE_STATE_DIRECTORY: node.dataPath,
              DASSIE_IPC_SOCKET_PATH: node.ipcSocketPath,
              DASSIE_DEV_ROOT: viteServer.config.root,
              DASSIE_DEV_BASE: viteServer.config.base,
              DASSIE_DEV_ENTRY: node.entry,
              DASSIE_DEV_RPC_URL: `wss://dev-rpc.localhost:${DEBUG_RPC_PORT}`,
              DASSIE_DEV_NODE_ID: node.id,
            } satisfies RunnerEnvironment,
            extraArguments: [`--inspect-port=${node.debugPort}`],
          })

          await setTimeout(NODE_STARTUP_INTERVAL)
        })
      )

    await sig.runMapSequential(nodeActorMap)
  })
