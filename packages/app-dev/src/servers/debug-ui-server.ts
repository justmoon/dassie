import serveStatic from "serve-static"
import { createServer } from "vite"
import type { ViteDevServer } from "vite"

import { createLogger } from "@xen-ilp/lib-logger"

const logger = createLogger("xen:dev:debug-ui-server")

const debugUiPath = new URL("../../", import.meta.url).pathname
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

export const startDebugUiServer = async () => {
  const server = await createServer({
    root: debugUiPath,
    server: {
      port,
    },
    plugins: [devtoolsServer()],
  })
  await server.listen(port)

  logger.info(`listening on http://localhost:${port}/`)
}
