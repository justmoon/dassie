import * as colors from "picocolors"

import { posix } from "node:path"

import { createLogger } from "@dassie/lib-logger"
import { EffectContext, createTopic } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"
import { viteService } from "../services/vite-server"

const logger = createLogger("das:dev:handle-file-change")

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? posix.relative(root, file) : file
}

export const fileChangeTopic = () => createTopic()

export const handleFileChange = async (sig: EffectContext) => {
  const viteServer = await sig.get(viteService)

  if (!viteServer) return

  const onFileChange = (file: string) => {
    const { config, moduleGraph } = viteServer
    const shortFile = getShortName(file, config.root)

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
