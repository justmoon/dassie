import { createServer } from "node:http"

import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../../../../config/database-config"
import { HttpRouter } from "../../../../http-server/values/http-router"
import { http as logger } from "../../../../logger/instances"

function handleError(error: unknown) {
  logger.error("http server error", { error })
}

export const ServeHttpActor = (reactor: Reactor) => {
  const router = reactor.use(HttpRouter)

  return createActor((sig) => {
    const { httpPort } = sig.readKeysAndTrack(DatabaseConfigStore, ["httpPort"])

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => router.handle(context),
      onError: handleError,
    })
    const server = createServer({})

    logger.debug?.("http server listening", { port: httpPort })

    server.listen(httpPort)

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
