import { createServer } from "vite"
import { ViteNodeRunner } from "vite-node/client"
import { ViteNodeServer } from "vite-node/server"
import { installSourcemapsSupport } from "vite-node/source-map"

const loadDevelopmentServer = async () => {
  const viteServer = await createServer({
    logLevel: "error",
    configFile: "packages/app-dev/vite.backend.config.js",
    define: {
      __DASSIE_VERSION__: '"dev"',
    },
    server: { hmr: false },
    clearScreen: false,
  })

  try {
    await viteServer.pluginContainer.buildStart({})

    const viteNodeServer = new ViteNodeServer(viteServer)

    installSourcemapsSupport({
      getSourceMap: (source) => viteNodeServer.getSourceMap(source),
    })

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

    /** @type import("../src") */
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    const startModule = await viteNodeRunner.executeFile("packages/app-dev/src")

    return {
      viteServer,
      viteNodeServer,
      viteNodeRunner,
      start: startModule.default,
    }
  } catch (error) {
    await viteServer.close()
    throw error
  }
}

const { viteServer, viteNodeServer, start } = await loadDevelopmentServer()

await start({ viteServer, viteNodeServer })

// interface GlobalWithReactor {
//   reactor?: Reactor | undefined
//   latest?: symbol
// }
// const context = global as GlobalWithReactor
// const ourSymbol = Symbol()
// context.latest = ourSymbol
// if (context.reactor) {
//   await context.reactor.dispose()
// }
// if (context.latest === ourSymbol) {
//   context.reactor = start()
// }
//           })
