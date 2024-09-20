import { readFileSync } from "node:fs"
import path from "node:path"

import {
  type HttpResponse,
  createBinaryResponse,
  createPlainResponse,
} from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { DassieReactor } from "../base/types/dassie-base"
import { EnvironmentConfig } from "../config/environment-config"
import { HttpsRouter } from "./values/https-router"

const ASSETS_REGEX = /^\/((?:assets\/)?[\w-]+\.([a-z]{2,}))$/

const MIME_TYPES_BY_EXTENSION: Record<string, string> = {
  ico: "image/x-icon",
  png: "image/png",
  css: "text/css",
  js: "text/javascript",
  webmanifest: "application/manifest+json",
}

export const ServeFrontendActor = (reactor: DassieReactor) => {
  const httpsRouter = reactor.use(HttpsRouter)
  const { rootPath } = reactor.use(EnvironmentConfig)

  const frontendPath = path.resolve(rootPath, "share/public")
  const indexHtmlPath = path.resolve(frontendPath, "index.html")

  function serveAsset(pathname: string): HttpResponse | undefined {
    const match = ASSETS_REGEX.exec(pathname)
    if (match) {
      const [, filename, extension] = match

      const contentType = MIME_TYPES_BY_EXTENSION[extension!]
      if (!contentType) return undefined

      const assetPath = path.resolve(frontendPath, filename!)

      try {
        return createBinaryResponse(readFileSync(assetPath), {
          contentType,
        })
      } catch {
        return undefined
      }
    }

    return undefined
  }

  return createActor((sig) => {
    // In development, the frontend is injected by the runner.
    // TODO: Not sure if that is still the best way to do it.
    if (process.env["NODE_ENV"] === "development") return

    httpsRouter
      // .get()
      .method("get")
      .path("*")
      .handler(sig, ({ url }) => {
        const assetResponse = serveAsset(url.pathname)
        if (assetResponse) return assetResponse

        return createPlainResponse(readFileSync(indexHtmlPath, "utf8"), {
          contentType: "text/html",
        })
      })
  })
}
