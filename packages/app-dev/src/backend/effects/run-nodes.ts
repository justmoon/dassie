import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

import { viteNodeService } from "../services/vite-node-server"
import { viteService } from "../services/vite-server"
import { activeTemplateSignal } from "../signals/active-template"
import { generateNodeConfig } from "../utils/generate-node-config"
import { fileChangeTopic } from "./handle-file-change"
import { runNodeChildProcess } from "./run-node-child-process"
import { CertificateInfo, validateCertificates } from "./validate-certificates"

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

export const runNodes = async (sig: EffectContext) => {
  const viteServer = await sig.get(viteService)
  const nodeServer = await sig.get(viteNodeService)

  if (!viteServer || !nodeServer) return

  // Restart child processes when a file changes
  sig.subscribe(fileChangeTopic)

  logger.debug("starting node processes")

  await Promise.all(
    sig.forIndex(activeTemplateSignal, async (sig, [peers, index]) => {
      const node = generateNodeConfig(index, peers)

      const neededCertificates: CertificateInfo[] = [
        {
          type: "web",
          certificatePath: node.config.tlsWebCertFile,
          keyPath: node.config.tlsWebKeyFile,
        },
        {
          type: "dassie",
          certificatePath: node.config.tlsDassieCertFile,
          keyPath: node.config.tlsDassieKeyFile,
        },
      ]
      await sig.run(validateCertificates, {
        id: node.id,
        certificates: neededCertificates,
      })

      await sig.run(runNodeChildProcess, {
        viteServer,
        nodeServer,
        node,
      })
    })
  )
}
