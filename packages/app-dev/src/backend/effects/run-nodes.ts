import * as colors from "picocolors"

import { posix } from "node:path"

import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createTopic } from "@xen-ilp/lib-reactive"

import { viteNodeServerValue } from "../services/vite-node-server"
import { viteServerValue } from "../services/vite-server"
import { activeTemplate } from "../stores/active-template"
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
  const template = sig.get(activeTemplate)

  const viteServer = await sig.get(viteServerValue)
  const nodeServer = await sig.get(viteNodeServerValue)

  logger.debug("starting node processes")

  await sig.use(async (sig) => {
    // Restart child processes when a file changes
    sig.subscribe(fileChangeTopic)

    for (const node of template) {
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
