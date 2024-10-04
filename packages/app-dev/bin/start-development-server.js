/* eslint-disable @dassie/no-top-level-mutables */
import chalk from "chalk"
import { createServer } from "vite"
import { ViteNodeRunner } from "vite-node/client"
import { ViteNodeServer } from "vite-node/server"
import { installSourcemapsSupport } from "vite-node/source-map"

const log = console.info

const DEVELOPMENT_SERVER_ENTRYPOINT = "packages/app-dev/src/start.ts"

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

const { viteServer, viteNodeServer } = await createViteServer()

const getStartModule = async () => {
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

  /** @type import("../src/start") */
  // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
  const startModule = await viteNodeRunner.executeFile(
    DEVELOPMENT_SERVER_ENTRYPOINT,
  )

  return startModule
}

let isRestartQueued = false
/** @type Promise<void> | undefined */
let startupPromise

const start = async () => {
  const { prepareReactor, RootActor } = await getStartModule()

  const reactor = prepareReactor({ viteServer, viteNodeServer })

  reactor.onCleanup(async () => {
    if (!isRestartQueued) {
      await viteServer.close()
    }
  })

  const rootActor = reactor.use(RootActor)
  startupPromise = rootActor
    .run(reactor)
    ?.catch((/** @type unknown */ error) => {
      console.error("error running development server root actor", { error })
    })

  return reactor
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

let reactor = await start()

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
  if (isRestartQueued) return
  isRestartQueued = true

  log(chalk.green("\n  Restarting development server...\n"))
  ;(async () => {
    try {
      await startupPromise
      await reactor.dispose()
    } catch (/** @type unknown */ error) {
      console.error("error shutting down development server", { error })
    } finally {
      isRestartQueued = false
    }

    viteNodeServer.fetchCache = new Map()

    reactor = await start()
  })().catch((/** @type unknown */ error) => {
    console.error(error)
  })
}
