import chalk from "chalk"
import { createServer } from "vite"
import { ViteNodeRunner } from "vite-node/client"
import { ViteNodeServer } from "vite-node/server"
import { installSourcemapsSupport } from "vite-node/source-map"

const log = console.info

const DEVELOPMENT_SERVER_ENTRYPOINT = "packages/app-dev/src/backend/start.ts"

const createViteServer = async () => {
  const viteServer = await createServer({
    logLevel: "error",
    configFile: "packages/app-dev/vite.backend.config.js",
    define: {
      __DASSIE_VERSION__: '"dev"',
    },
    server: {
      hmr: false,
      watch: {
        ignored: [
          "**/dist/**",
          "**/.next/**",
          "**/.turbo/**",
          "**/.cache/**",
          "**/.vagrant/**",
        ],
      },
    },
    clearScreen: false,
  })

  try {
    await viteServer.pluginContainer.buildStart({})

    const viteNodeServer = new ViteNodeServer(viteServer)

    installSourcemapsSupport({
      getSourceMap: (source) => viteNodeServer.getSourceMap(source),
    })

    return {
      viteServer,
      viteNodeServer,
    }
  } catch (error) {
    await viteServer.close()
    throw error
  }
}

const getStartModule = async (
  /** @type import("vite").ViteDevServer */
  viteServer,
  /** @type ViteNodeServer */
  viteNodeServer,
) => {
  const viteNodeRunner = new ViteNodeRunner({
    root: viteServer.config.root,
    base: viteServer.config.base,
    // when having the server and runner in a different context,
    // you will need to handle the communication between them
    // and pass to this function
    fetchModule(id) {
      return viteNodeServer.fetchModule(id)
    },
    resolveId(id, importer) {
      return viteNodeServer.resolveId(id, importer)
    },
  })

  /** @type import("../src/backend/start") */
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
  const startModule = await viteNodeRunner.executeFile(
    DEVELOPMENT_SERVER_ENTRYPOINT,
  )

  return startModule.default
}

const isWithinBoundary = (
  /** @type import('vite').ModuleNode */ node,
  /** @type Set<import('vite').ModuleNode> */ traversedModules,
  /** @type Set<import('vite').ModuleNode> */ boundaryModules,
) => {
  if (traversedModules.has(node)) {
    return false
  }
  traversedModules.add(node)

  if (boundaryModules.has(node)) {
    return true
  }

  for (const importer of node.importers) {
    if (isWithinBoundary(importer, traversedModules, boundaryModules)) {
      return true
    }
  }

  return false
}

log(chalk.bold(`  Dassie${chalk.green("//dev")}\n`))
log("  Starting development server...\n")

const { viteServer, viteNodeServer } = await createViteServer()

const start = await getStartModule(viteServer, viteNodeServer)
let reactorPromise = start({ viteServer, viteNodeServer })
let isRestartInProgress = false

viteServer.watcher.on("change", (/** @type string */ file) => {
  const { moduleGraph } = viteServer

  const entrypoint = `${process.cwd()}/${DEVELOPMENT_SERVER_ENTRYPOINT}`

  const changedModules = moduleGraph.getModulesByFile(file)
  const boundaryModules = moduleGraph.getModulesByFile(entrypoint)
  if (!changedModules || !boundaryModules) return

  /**@type Set<import('vite').ModuleNode> */
  const traversedModules = new Set()
  for (const module of changedModules) {
    if (isWithinBoundary(module, traversedModules, boundaryModules)) {
      restart()
    }
  }
})

function restart() {
  if (isRestartInProgress) return
  isRestartInProgress = true

  log(chalk.green("\n  Restarting development server...\n"))
  ;(async () => {
    const reactor = await reactorPromise
    await reactor.dispose()

    viteNodeServer.fetchCache = new Map()

    const start = await getStartModule(viteServer, viteNodeServer)
    reactorPromise = start({ viteServer, viteNodeServer })
  })()
    .catch((/** @type unknown */ error) => {
      console.error("error in development server", { error })
    })
    .finally(() => {
      isRestartInProgress = false
    })
}
