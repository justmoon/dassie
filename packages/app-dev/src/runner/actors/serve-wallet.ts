import type { NextHandleFunction } from "connect"
import history from "connect-history-api-fallback"
import { type ViteDevServer, createServer } from "vite"

import assert from "node:assert"
import { existsSync, readFileSync } from "node:fs"
import { join, normalize } from "node:path"

import { DatabaseConfigStore } from "@dassie/app-node/src/backend/config/database-config"
import { AdditionalMiddlewaresSignal } from "@dassie/app-node/src/backend/http-server/serve-https"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { runner as logger } from "../../backend/logger/instances"

const walletPath = new URL("../../../../app-node/src/frontend", import.meta.url)
  .pathname

const FS_PREFIX = `/@fs/`
const VOLUME_RE = /^[a-z]:/i

const queryRE = /\?.*$/s
const hashRE = /#.*$/s

function fsPathFromId(id: string): string {
  const fsPath = normalize(
    id.startsWith(FS_PREFIX) ? id.slice(FS_PREFIX.length) : id,
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

export const ServeWalletActor = (reactor: Reactor) => {
  const additionalMiddlewaresSignal = reactor.use(AdditionalMiddlewaresSignal)

  return createActor(async (sig) => {
    const { httpsPort, tlsWebCert, tlsWebKey } = sig.getKeys(
      DatabaseConfigStore,
      ["httpsPort", "tlsWebCert", "tlsWebKey"],
    )

    assert(tlsWebCert, "Web UI is not configured, missing certificate")
    assert(tlsWebKey, "Web UI is not configured, missing private key")

    const server = await createServer({
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
          cert: tlsWebCert,
          key: tlsWebKey,
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
      next,
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
            html = await server.transformIndexHtml(
              url,
              html,
              request.originalUrl,
            )
            response.writeHead(200, { "Content-Type": "text/html" })
            response.end(html)
          } catch (error: unknown) {
            return next(error)
          }
        }
      }
      next()
    }

    additionalMiddlewaresSignal.update((state) => [
      ...state,
      server.middlewares,
      historySpaFallbackMiddleware,
      serveIndexHtml,
    ])

    sig.onCleanup(async () => {
      additionalMiddlewaresSignal.update((state) =>
        state.filter(
          (middleware) =>
            ![
              server.middlewares,
              historySpaFallbackMiddleware,
              serveIndexHtml,
            ].includes(middleware),
        ),
      )

      await server.close()
    })

    logger.info(`serving wallet ui`)
  })
}
