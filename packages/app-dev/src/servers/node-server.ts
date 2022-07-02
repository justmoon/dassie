import colors from "picocolors"
import { createServer } from "vite"
import { ViteNodeServer } from "vite-node/server"

import { posix } from "node:path"

import type { EventBroker } from "@xen-ilp/lib-events"
import { createLogger } from "@xen-ilp/lib-logger"

import ChildProcessWrapper from "../classes/child-process-wrapper"

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

export interface NodeServerContext {
  eventBroker: EventBroker
}

export default class NodeServer {
  constructor(readonly context: NodeServerContext) {}

  async start<T>(nodes: NodeDefinition<T>[]) {
    // create vite server
    const server = await createServer({
      server: { hmr: false },
      optimizeDeps: {
        // It's recommended to disable deps optimization
        disabled: true,
      },
    })

    // this is needed to initialize the plugins
    await server.pluginContainer.buildStart({})

    // create vite-node server
    const nodeServer = new ViteNodeServer(server)

    const processes = nodes.map((node) => {
      return new ChildProcessWrapper(this.context, server, nodeServer, node)
    })

    logger.debug("starting node processes")

    for (const process of processes) {
      try {
        await process.start()
      } catch (error) {
        logger.logError(error)
        logger.error(
          `${colors.red(
            `node ${process.node.id} failed, skip starting other nodes`
          )}`
        )
        break
      }
    }

    server.watcher.on("change", async (file) => {
      const { config, moduleGraph } = server
      const shortFile = getShortName(file, config.root)

      const mods = moduleGraph.getModulesByFile(file)

      try {
        if (mods && mods.size > 0) {
          logger.clear()
          logger.info(
            `${colors.green(`restart nodes`)} ${colors.dim(shortFile)}`
          )

          // Stop all processes in parallel
          await Promise.all(processes.map((runner) => runner.stop()))

          // Start each process one at a time unless one fails
          for (const proc of processes) {
            try {
              await proc.start()
            } catch {
              logger.error(
                `${colors.red(
                  `node ${proc.node.id} failed, skip starting other nodes`
                )}`
              )
              break
            }
          }
        }
      } catch (error) {
        logger.logError(error)
      }
    })

    process.on("SIGINT", async () => {
      await Promise.all(processes.map((runner) => runner.stop()))
      setTimeout(() => process.exit(), 1000)
    })
  }
}
