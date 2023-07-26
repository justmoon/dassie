import chalk from "chalk"

import { posix } from "node:path"

import { createActor, createTopic } from "@dassie/lib-reactive"

import { logsStore } from "../../common/stores/logs"
import { vite as logger } from "../logger/instances"
import { viteNodeService } from "../services/vite-node-server"
import { viteService } from "../services/vite-server"

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? posix.relative(root, file) : file
}

export const fileChangeTopic = () => createTopic()

export const handleFileChange = () =>
  createActor((sig) => {
    const viteServer = sig.get(viteService)
    const viteNodeServer = sig.get(viteNodeService)

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
        logger.info(`${chalk.green(`change`)} ${chalk.dim(shortFile)}`)

        sig.use(fileChangeTopic).emit(undefined)
      }
    }

    viteServer.watcher.on("change", onFileChange)

    sig.onCleanup(() => {
      viteServer.watcher.off("change", onFileChange)
    })
  })
