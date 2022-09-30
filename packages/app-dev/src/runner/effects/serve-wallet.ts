import type { NextHandleFunction } from "connect"
import history from "connect-history-api-fallback"
import { ViteDevServer, createServer } from "vite"

import { existsSync, readFileSync } from "node:fs"
import { join, normalize } from "node:path"

import { configSignal } from "@dassie/app-node"
import { additionalMiddlewaresSignal } from "@dassie/app-node/src/http-server/serve-http"
import { createLogger } from "@dassie/lib-logger"
import type { EffectContext } from "@dassie/lib-reactive"

const logger = createLogger("das:dev:runner:wallet-server")

const walletPath = new URL("../../../../app-wallet", import.meta.url).pathname

const FS_PREFIX = `/@fs/`
const VOLUME_RE = /^[a-z]:/i

const queryRE = /\?.*$/s
const hashRE = /#.*$/s

function fsPathFromId(id: string): string {
  const fsPath = normalize(
    id.startsWith(FS_PREFIX) ? id.slice(FS_PREFIX.length) : id
  )
  return fsPath.startsWith("/") || VOLUME_RE.test(fsPath)
    ? fsPath
    : `/${fsPath}`
}

const cleanUrl = (url: string): string =>
  url.replace(hashRE, "").replace(queryRE, "")

function getHtmlFilename(url: string, server: ViteDevServer) {
  return url.startsWith(FS_PREFIX)
    ? decodeURIComponent(fsPathFromId(url))
    : decodeURIComponent(normalize(join(server.config.root, url.slice(1))))
}

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

  const historySpaFallbackMiddleware = history({
    // support /dir/ without explicit index.html
    rewrites: [
      {
        from: /\/$/,
        to({ parsedUrl }) {
          const rewritten =
            decodeURIComponent(parsedUrl.pathname!) + "index.html"

          return existsSync(join(walletPath, rewritten))
            ? rewritten
            : `/index.html`
        },
      },
    ],
  }) as NextHandleFunction

  // eslint-disable-next-line @typescript-eslint/no-misused-promises
  const serveIndexHtml: NextHandleFunction = async (
    request,
    response,
    next
  ) => {
    if (response.writableEnded) {
      return next()
    }

    const url = request.url && cleanUrl(request.url)
    // spa-fallback always redirects to /index.html
    if (
      url?.endsWith(".html") &&
      request.headers["sec-fetch-dest"] !== "script"
    ) {
      const filename = getHtmlFilename(url, server)
      if (existsSync(filename)) {
        try {
          let html = readFileSync(filename, "utf8")
          html = await server.transformIndexHtml(url, html, request.originalUrl)
          response.writeHead(200, { "Content-Type": "text/html" })
          response.end(html)
        } catch (error: unknown) {
          return next(error)
        }
      }
    }
    next()
  }

  additionalMiddlewares.update((state) => [
    ...state,
    server.middlewares,
    historySpaFallbackMiddleware,
    serveIndexHtml,
  ])

  sig.onCleanup(async () => {
    additionalMiddlewares.update((state) =>
      state.filter(
        (middleware) =>
          ![
            server.middlewares,
            historySpaFallbackMiddleware,
            serveIndexHtml,
          ].includes(middleware)
      )
    )

    await server.close()
  })

  logger.info(`serving wallet ui`)
}
