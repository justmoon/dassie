import type { NextHandleFunction } from "connect"
import { createServer } from "vite"

import { readFileSync } from "node:fs"
import { resolve } from "node:path"

import { configSignal } from "@dassie/app-node"
import { additionalMiddlewaresSignal } from "@dassie/app-node/src/http-server/serve-http"
import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

const logger = createLogger("das:dev:runner:wallet-server")

const walletPath = new URL("../../../../app-wallet", import.meta.url).pathname

export const serveWallet = async (sig: EffectContext) => {
  const additionalMiddlewares = sig.use(additionalMiddlewaresSignal)
  const { host, port, tlsWebCert, tlsWebKey } = sig.getKeys(configSignal, [
    "host",
    "port",
    "tlsWebCert",
    "tlsWebKey",
  ])

  const server = await createServer({
    root: walletPath,
    mode: "development",
    appType: "custom",
    server: {
      middlewareMode: true,
      hmr: {
        port: port + 4400,
      },
      https: {
        cert: tlsWebCert,
        key: tlsWebKey,
      },
    },
    define: {
      __DASSIE_NODE_URL__: `'https://${host}:${port}/'`,
    },
  })

  // eslint-disable-next-line @typescript-eslint/no-misused-promises
  const serveIndexHtml: NextHandleFunction = async (
    request,
    response,
    next
  ) => {
    const url = request.originalUrl!

    try {
      let html = readFileSync(resolve(walletPath, "index.html"), "utf8")

      html = await server.transformIndexHtml(url, html)

      response.writeHead(200, { "Content-Type": "text/html" })
      response.end(html)
    } catch (error: unknown) {
      server.ssrFixStacktrace(error as Error)
      next(error)
    }
  }

  additionalMiddlewares.update((state) => [
    ...state,
    server.middlewares,
    serveIndexHtml,
  ])

  sig.onCleanup(() => {
    additionalMiddlewares.update((state) =>
      state.filter(
        (middleware) =>
          middleware !== server.middlewares && middleware !== serveIndexHtml
      )
    )
  })

  logger.info(`serving wallet ui`)
}
