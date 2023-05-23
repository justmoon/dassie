import { createLogger } from "@dassie/lib-logger"
import { createActor } from "@dassie/lib-reactive"

import { viteNodeService } from "../services/vite-node-server"
import { viteService } from "../services/vite-server"
import { activeNodesStore } from "../stores/active-nodes"
import type { generateNodeConfig } from "../utils/generate-node-config"
import { prepareDataDirectory } from "../utils/prepare-data-directory"
import {
  type CertificateInfo,
  validateCertificates,
} from "../utils/validate-certificates"
import { fileChangeTopic } from "./handle-file-change"
import { runNodeChildProcess } from "./run-node-child-process"

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

    const nodeActor = () =>
      createActor(async (sig, node: ReturnType<typeof generateNodeConfig>) => {
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

        // Prepare data directory
        {
          const { dataPath } = node.config

          await prepareDataDirectory(dataPath)
        }

        await sig.run(runNodeChildProcess, {
          viteServer,
          nodeServer,
          node,
        }).result
      })

    await Promise.all(sig.for(activeNodesStore, nodeActor))
  })
