import history from "connect-history-api-fallback"
import express, { type RequestHandler } from "express"
import { type ViteDevServer, createServer as createViteServer } from "vite"

import { existsSync, readFileSync } from "node:fs"
import type { IncomingMessage, ServerResponse } from "node:http"
import { createServer } from "node:https"
import path from "node:path"

import type { DassieReactor } from "@dassie/app-dassie/src/base/types/dassie-base"
import { DatabaseConfigStore } from "@dassie/app-dassie/src/config/database-config"
import { FallbackToSelfSigned } from "@dassie/app-dassie/src/http-server/functions/fallback-to-self-signed"
import { HttpsRouter } from "@dassie/app-dassie/src/http-server/values/https-router"
import { HttpsWebSocketRouter } from "@dassie/app-dassie/src/http-server/values/https-websocket-router"
import { http as logger } from "@dassie/app-dassie/src/logger/instances"
import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

type ExpressMiddleware = RequestHandler

const walletPath = new URL("../../../../gui-dassie", import.meta.url).pathname

function handleError(error: unknown) {
  logger.error("https server error", { error })
}

const FS_PREFIX = `/@fs/`
const VOLUME_RE = /^[a-z]:/i

const queryRE = /\?.*$/s
const hashRE = /#.*$/s

function fsPathFromId(id: string): string {
  const fsPath = path.normalize(
    id.startsWith(FS_PREFIX) ? id.slice(FS_PREFIX.length) : id,
  )
  return fsPath.startsWith("/") || VOLUME_RE.test(fsPath) ?
      fsPath
    : `/${fsPath}`
}

const cleanUrl = (url: string): string =>
  url.replace(hashRE, "").replace(queryRE, "")

function getHtmlFilename(url: string, server: ViteDevServer) {
  return url.startsWith(FS_PREFIX) ?
      decodeURIComponent(fsPathFromId(url))
    : decodeURIComponent(
        path.normalize(path.join(server.config.root, url.slice(1))),
      )
}

export const ServeHttpsActor = (reactor: DassieReactor) => {
  const httpsRouter = reactor.use(HttpsRouter)
  const websocketRouter = reactor.use(HttpsWebSocketRouter)
  const fallbackToSelfSigned = reactor.use(FallbackToSelfSigned)

  return createActor(async (sig) => {
    const { httpsPort, url, tlsWebCert, tlsWebKey } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpsPort", "url", "tlsWebCert", "tlsWebKey"],
    )

    const { tlsWebCert: cert, tlsWebKey: key } = await fallbackToSelfSigned({
      tlsWebCert,
      tlsWebKey,
    })

    const viteServer = await createViteServer({
      root: walletPath,
      mode: "development",
      appType: "custom",
      define: {
        __DASSIE_VERSION__: '"dev"',
      },
      server: {
        middlewareMode: true,
        hmr: {
          port: httpsPort + 4400,
        },
        https: {
          cert,
          key,
        },
        cors: false,
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

            return existsSync(path.join(walletPath, rewritten)) ? rewritten : (
                `/index.html`
              )
          },
        },
      ],
    }) as ExpressMiddleware

    // eslint-disable-next-line @typescript-eslint/no-misused-promises
    const serveIndexHtml: ExpressMiddleware = async (
      request,
      response,
      next,
    ) => {
      if (response.writableEnded) {
        next()
        return
      }

      const url = request.url && cleanUrl(request.url)
      // spa-fallback always redirects to /index.html
      if (
        url.endsWith(".html") &&
        request.header("sec-fetch-dest") !== "script"
      ) {
        const filename = getHtmlFilename(url, viteServer)
        if (existsSync(filename)) {
          try {
            let html = readFileSync(filename, "utf8")
            html = await viteServer.transformIndexHtml(
              url,
              html,
              request.originalUrl,
            )
            response.writeHead(200, { "Content-Type": "text/html" })
            response.end(html)
          } catch (error: unknown) {
            next(error)
            return
          }
        }
      }
      next()
    }

    const app = express()

    app.use(viteServer.middlewares)
    app.use(historySpaFallbackMiddleware)
    app.use(serveIndexHtml)

    const server = createServer({
      cert,
      key,
    })

    server.listen(httpsPort)

    logger.info(`listening on ${url}`)

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => httpsRouter.handle(context),
      onUpgrade: async (context) => websocketRouter.handle(context),
      onError: handleError,
    })

    function handleRequest(
      nodeRequest: IncomingMessage,
      nodeResponse: ServerResponse,
    ) {
      if (httpsRouter.match(nodeRequest.method!, nodeRequest.url!)) {
        nodejsHandlers.handleRequest(nodeRequest, nodeResponse)
      } else {
        app(nodeRequest, nodeResponse)
      }
    }

    server.addListener("request", handleRequest)
    server.addListener("upgrade", nodejsHandlers.handleUpgrade)
    server.addListener("error", nodejsHandlers.handleError)

    sig.onCleanup(async () => {
      await viteServer.close()
      server.removeListener("request", handleRequest)
      server.removeListener("upgrade", nodejsHandlers.handleUpgrade)
      server.removeListener("error", nodejsHandlers.handleError)
      server.close()
    })

    return server
  })
}
