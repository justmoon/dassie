import * as colors from "picocolors"

import { posix } from "node:path"

import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"
import { viteNodeService } from "../services/vite-node-server"
import { viteService } from "../services/vite-server"

const logger = createLogger("das:dev:handle-file-change")

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? posix.relative(root, file) : file
}

export const fileChangeTopic = () => createTopic()

export const handleFileChange = async (sig: EffectContext) => {
  const viteServer = await sig.get(viteService)
  const viteNodeServer = await sig.get(viteNodeService)

  if (!viteServer) return

  const onFileChange = (file: string) => {
    const { config, moduleGraph } = viteServer
    const shortFile = getShortName(file, config.root)

    // The cache in vite-node doesn't correctly invalidate, so we need to clear it on each file change.
    // It still helps a lot with performance because we are compiling the code once and then re-using it for every running process.
    if (viteNodeServer) viteNodeServer.fetchCache = new Map()

    const mods = moduleGraph.getModulesByFile(file)

    if (mods && mods.size > 0) {
      sig.use(logsStore).clear()
      logger.info(`${colors.green(`change`)} ${colors.dim(shortFile)}`)

      sig.use(fileChangeTopic).emit(undefined)
    }
  }

  viteServer.watcher.on("change", onFileChange)

  sig.onCleanup(() => {
    viteServer.watcher.off("change", onFileChange)
  })
}
