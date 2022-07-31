import { bold, dim } from "picocolors"
import serveStatic from "serve-static"
import { createServer } from "vite"
import type { ViteDevServer } from "vite"

import { createLogger } from "@xen-ilp/lib-logger"
import type { EffectContext } from "@xen-ilp/lib-reactive"

const logger = createLogger("xen:dev:debug-ui-server")

const debugUiPath = new URL("../../../", import.meta.url).pathname
const port = 10_000

const devtoolsServer = () => ({
  name: "html-ext-fallback",
  configureServer(server: ViteDevServer) {
    const devtoolsPath = new URL(
      "../../node_modules/chrome-devtools-frontend-prebuilt/public",
      import.meta.url
    ).pathname

    server.middlewares.use("/devtools", serveStatic(devtoolsPath, {}))
  },
})

export const debugUiServer = async (sig: EffectContext) => {
  const server = await createServer({
    root: debugUiPath,
    server: {
      port,
    },
    plugins: [devtoolsServer()],
  })
  await server.listen(port)

  console.log(
    `  ${bold("Debug UI:")} http://localhost:${port}/ ${dim(
      "<-- Start here"
    )}\n`
  )
  logger.info(`listening on http://localhost:${port}/`)

  sig.onCleanup(async () => {
    await server.close()
  })
}
