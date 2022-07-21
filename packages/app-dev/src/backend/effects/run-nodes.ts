import * as colors from "picocolors"

import { posix } from "node:path"

import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createTopic } from "@xen-ilp/lib-reactive"

import { NODES } from "../constants/development-nodes"
import { viteNodeServerFactory } from "../services/vite-node-server"
import { viteServerFactory } from "../services/vite-server"
import { runNodeChildProcess } from "./run-node-child-process"

const logger = createLogger("xen:dev:node-server")

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? posix.relative(root, file) : file
}

export interface NodeDefinition<T> {
  id: string
  port: number
  debugPort: number
  peers: string[]
  config: T
  url: string
  entry?: string
}

const fileChangeTopic = () => createTopic()

export const runNodes = async (sig: EffectContext) => {
  const viteServer = await sig.reactor.fromContext(viteServerFactory)
  const nodeServer = await sig.reactor.fromContext(viteNodeServerFactory)

  logger.debug("starting node processes")

  await sig.use(async (sig) => {
    // Restart child processes when a file changes
    sig.subscribe(fileChangeTopic)

    for (const node of NODES) {
      await sig.use(runNodeChildProcess, { viteServer, nodeServer, node })
    }
  })

  const handleFileChange = (file: string) => {
    const { config, moduleGraph } = viteServer
    const shortFile = getShortName(file, config.root)

    const mods = moduleGraph.getModulesByFile(file)

    if (mods && mods.size > 0) {
      logger.clear()
      logger.info(`${colors.green(`restart nodes`)} ${colors.dim(shortFile)}`)

      sig.emit(fileChangeTopic, undefined)
    }
  }

  viteServer.watcher.on("change", handleFileChange)

  sig.onCleanup(() => {
    viteServer.watcher.off("change", handleFileChange)
  })
}
