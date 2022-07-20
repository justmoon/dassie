import * as colors from "picocolors"
import { createServer } from "vite"
import { ViteNodeServer } from "vite-node/server"

import { posix } from "node:path"

import { createLogger } from "@xen-ilp/lib-logger"
import { EffectContext, createTopic } from "@xen-ilp/lib-reactive"

import { NODES } from "../constants/development-nodes"
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
  // create vite server
  const viteServer = await createServer({
    server: { hmr: false },
    optimizeDeps: {
      // It's recommended to disable deps optimization
      disabled: true,
    },
  })

  // this is needed to initialize the plugins
  await viteServer.pluginContainer.buildStart({})

  // create vite-node server
  const nodeServer = new ViteNodeServer(viteServer)

  logger.debug("starting node processes")

  await sig.use(async (sig) => {
    // Restart child processes when a file changes
    sig.subscribe(fileChangeTopic)

    for (const node of NODES) {
      await sig.use(runNodeChildProcess, { viteServer, nodeServer, node })
    }
  })

  viteServer.watcher.on("change", (file) => {
    const { config, moduleGraph } = viteServer
    const shortFile = getShortName(file, config.root)

    const mods = moduleGraph.getModulesByFile(file)

    if (mods && mods.size > 0) {
      logger.clear()
      logger.info(`${colors.green(`restart nodes`)} ${colors.dim(shortFile)}`)

      sig.emit(fileChangeTopic, undefined)
    }
  })

  sig.onCleanup(async () => {
    await viteServer.close()
  })
}
