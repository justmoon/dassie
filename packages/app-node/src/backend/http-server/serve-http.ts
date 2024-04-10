import { createServer } from "node:http"

import {
  NotFoundFailure,
  RedirectResponse,
  createNodejsHttpHandlers,
  createRouter,
} from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { HasTlsSignal } from "../config/computed/has-tls"
import { DatabaseConfigStore } from "../config/database-config"
import { http as logger } from "../logger/instances"
import { getListenTargets } from "./utils/listen-targets"

export const HttpRouter = () => createRouter()

function handleError(error: unknown) {
  logger.error("http server error", { error })
}

export const ServeHttpActor = (reactor: Reactor) => {
  const router = reactor.use(HttpRouter)

  return createActor((sig) => {
    const { httpPort, httpsPort, enableHttpServer } = sig.readKeysAndTrack(
      DatabaseConfigStore,
      ["httpPort", "httpsPort", "enableHttpServer"],
    )
    const hasTls = sig.readAndTrack(HasTlsSignal)

    if (!enableHttpServer) return

    // TODO: Maybe show some helpful message if TLS is not yet set up
    router
      .get()
      .path("*")
      .handler(sig, ({ url }) => {
        if (hasTls) {
          // Redirect to HTTPS
          return new RedirectResponse(
            `https://${url.hostname}${
              httpsPort === 443 ? "" : `:${httpsPort}`
            }${url.pathname}`,
            301,
          )
        }

        return new NotFoundFailure("Not Found")
      })

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => router.handle(context),
      onError: handleError,
    })
    const server = createServer({})

    for (const listenTarget of getListenTargets(httpPort, false)) {
      server.listen(listenTarget)
    }

    server.addListener("request", nodejsHandlers.handleRequest)
    server.addListener("error", nodejsHandlers.handleError)

    sig.onCleanup(() => {
      server.removeListener("request", nodejsHandlers.handleRequest)
      server.removeListener("error", nodejsHandlers.handleError)
      server.close()
    })

    return server
  })
}
