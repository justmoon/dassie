import colors from "picocolors"
import { createLogger, createServer } from "vite"
import { ViteNodeServer } from "vite-node/server"

import { posix } from "node:path"

import { NODES } from "../constants/dev-nodes"
import DevProcess from "./dev-process"
import DevRpcHost from "./dev-rpc-host"

export function getShortName(file: string, root: string): string {
  return file.startsWith(root + "/") ? posix.relative(root, file) : file
}

export const startDevServer = async () => {
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

  const rpcHost = new DevRpcHost(nodeServer)

  const runners = NODES.map(({ nodeId, config }) => {
    return new DevProcess(nodeId, config, server, rpcHost)
  })

  logger.info(
    `  \n${colors.cyan("xen")} ${colors.green("dev server running")}\n  `,
    { clear: true }
  )

  for (const runner of runners) {
    await runner.start()
  }

  server.watcher.on("change", async (file) => {
    const { config, moduleGraph } = server
    const shortFile = getShortName(file, config.root)

    const mods = moduleGraph.getModulesByFile(file)

    try {
      if (mods && mods.size) {
        logger.info(
          `${colors.green(`restart nodes`)} ${colors.dim(shortFile)}`,
          {
            clear: true,
            timestamp: true,
          }
        )
        await Promise.all(runners.map((runner) => runner.restart()))
      }
    } catch (err) {
      console.error(err)
    }
  })
}
