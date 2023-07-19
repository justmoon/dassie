import type { NextHandleFunction } from "connect"
import { type Request, type Response, static as serveStatic } from "express"

import { resolve } from "node:path"

import { createActor } from "@dassie/lib-reactive"

import { environmentConfigSignal } from "../config/environment-config"
import { additionalMiddlewaresSignal } from "./serve-https"

export const serveFrontend = () =>
  createActor((sig) => {
    // In development, the frontend is injected by the runner.
    // TODO: Not sure if that is still the best way to do it.
    if (process.env["NODE_ENV"] === "development") return

    const { rootPath } = sig.getKeys(environmentConfigSignal, ["rootPath"])
    const frontendPath = resolve(rootPath, "share/public")

    const middleware = sig.use(additionalMiddlewaresSignal)
    const staticMiddleware = serveStatic(frontendPath, {
      index: false,
    }) as NextHandleFunction
    const indexMiddleware = ((_request: Request, response: Response) => {
      response.sendFile(resolve(frontendPath, "index.html"))
    }) as unknown as NextHandleFunction

    middleware.update((middlewares) => [
      ...middlewares,
      staticMiddleware,
      indexMiddleware,
    ])

    sig.onCleanup(() => {
      middleware.update((middlewares) =>
        middlewares.filter(
          (middleware) =>
            middleware !== staticMiddleware && middleware !== indexMiddleware
        )
      )
    })
  })
