import colors from "picocolors"
import { createLogger, createServer } from "vite"
import { ViteNodeServer } from "vite-node/server"

import { posix } from "node:path"

import ChildProcessWrapper from "./classes/child-process-wrapper"

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? posix.relative(root, file) : file
}

export interface NodeDefinition<T> {
  id: string
  config: T
  url: string
  entry?: string
}

export const startDevelopmentServer = async <T>(
  nodes: Array<NodeDefinition<T>>
) => {
  const logger = createLogger("info", {
    prefix: "[dev]",
  })

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
    return new ChildProcessWrapper(server, nodeServer, node)
  })

  logger.info(
    `  \n${colors.cyan("xen")} ${colors.green("dev server running")}\n  `,
    { clear: true }
  )

  for (const proc of processes) {
    await proc.start()
  }

  server.watcher.on("change", async (file) => {
    const { config, moduleGraph } = server
    const shortFile = getShortName(file, config.root)

    const mods = moduleGraph.getModulesByFile(file)

    try {
      if (mods && mods.size > 0) {
        logger.info(
          `${colors.green(`restart nodes`)} ${colors.dim(shortFile)}`,
          {
            clear: true,
            timestamp: true,
          }
        )
        await Promise.all(processes.map((runner) => runner.restart()))
      }
    } catch (error) {
      console.error(error)
    }
  })
}
