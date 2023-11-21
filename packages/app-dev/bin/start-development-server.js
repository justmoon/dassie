import chalk from "chalk"
import { createServer } from "vite"
import { ViteNodeRunner } from "vite-node/client"
import { ViteNodeServer } from "vite-node/server"
import { installSourcemapsSupport } from "vite-node/source-map"

const log = console.info

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
    "packages/app-dev/src/backend/start.ts",
  )

  return startModule.default
}

log(chalk.bold(`  Dassie${chalk.green("//dev")}\n`))
log("  Starting development server...\n")

const { viteServer, viteNodeServer } = await createViteServer()

const start = await getStartModule(viteServer, viteNodeServer)
let reactorPromise = start({ viteServer, viteNodeServer, restart })
let isRestartInProgress = false

function restart() {
  if (isRestartInProgress) return
  isRestartInProgress = true

  log(chalk.green("\n  Restarting development server...\n"))
  ;(async () => {
    const reactor = await reactorPromise
    await reactor.dispose()

    viteNodeServer.fetchCache = new Map()

    const start = await getStartModule(viteServer, viteNodeServer)
    reactorPromise = start({ viteServer, viteNodeServer, restart })
    isRestartInProgress = false
  })().catch((/** @type unknown */ error) => {
    console.error("error in development server", { error })
  })
}
