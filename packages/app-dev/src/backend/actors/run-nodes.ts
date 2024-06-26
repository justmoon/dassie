import { setTimeout } from "node:timers/promises"

import { Reactor, createActor, createMapped } from "@dassie/lib-reactive"

import { RunnerEnvironment } from "../../common/types/runner-environment"
import { ActiveNodesComputed } from "../computed/active-nodes"
import { DEBUG_UI_RPC_PORT } from "../constants/ports"
import { RunChildProcess } from "../functions/run-child-process"
import { children as logger } from "../logger/instances"
import { DebugScopesSignal } from "../signals/debug-scopes"
import { SecurityTokenSignal } from "../signals/security-token"
import { ScenarioStore } from "../stores/scenario"
import { ViteNodeServer } from "../unconstructables/vite-node-server"
import { ViteServer } from "../unconstructables/vite-server"
import { generateNodeConfig } from "../utils/generate-node-config"
import { prefillDatabase } from "../utils/prefill-database"
import { prepareDataDirectory } from "../utils/prepare-data-directory"
import {
  type CertificateInfo,
  validateCertificates,
} from "../utils/validate-certificates"

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

export const RunNodesActor = (reactor: Reactor) => {
  const viteServer = reactor.use(ViteServer)
  const viteNodeServer = reactor.use(ViteNodeServer)
  const runChildProcess = reactor.use(RunChildProcess)

  return createActor(async (sig) => {
    logger.info("starting node processes")

    // eslint-disable-next-line unicorn/consistent-function-scoping
    const NodeActorsMapped = (reactor: Reactor) =>
      createMapped(reactor, ActiveNodesComputed, (nodeIndex) =>
        createActor(async (sig) => {
          const { nodeSettings, environmentSettings } = sig.readAndTrack(
            ScenarioStore,
            ({ nodes, environment }) => ({
              nodeSettings: nodes[nodeIndex],
              environmentSettings: environment,
            }),
            (a, b) =>
              a.nodeSettings !== b.nodeSettings ||
              a.environmentSettings !== b.environmentSettings,
          )
          logger.assert(
            !!nodeSettings,
            `Settings for node ${nodeIndex} not found`,
          )

          const node = generateNodeConfig(
            nodeIndex,
            nodeSettings,
            environmentSettings,
          )

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

          const debugScopes = sig.readAndTrack(DebugScopesSignal)
          await runChildProcess({
            lifecycle: sig.lifecycle,
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
              DASSIE_DEV_SECURITY_TOKEN: sig.read(SecurityTokenSignal),
            } satisfies RunnerEnvironment,
            extraArguments: [
              "--stack-trace-limit=64",
              "--trace-deprecation",
              `--inspect-port=${node.debugPort}`,
            ],
          })

          await setTimeout(NODE_STARTUP_INTERVAL)
        }),
      )

    try {
      await sig.runMapSequential(NodeActorsMapped)
    } catch (error) {
      logger.error("error starting nodes", { error })
    }
  })
}
