import { createServer } from "node:http"

import { createNodejsHttpHandlers, createRouter } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { SystemdReactor } from "."
import { DatabaseConfigStore } from "../../../../backend/config/database-config"
import { http as logger } from "../../../../backend/logger/instances"
import { getSocketActivationFileDescriptors } from "./socket-activation"

export const HttpRouter = () => createRouter()

function handleError(error: unknown) {
  logger.error("http server error", { error })
}

export const SOCKET_ACTIVATION_NAME_HTTP = "dassie-http.socket"

export const ServeHttpActor = (reactor: SystemdReactor) => {
  const router = reactor.use(HttpRouter)

  return createActor((sig) => {
    const { enableHttpServer } = sig.readKeysAndTrack(DatabaseConfigStore, [
      "enableHttpServer",
    ])

    if (!enableHttpServer) return

    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => router.handle(context),
      onError: handleError,
    })
    const server = createServer({})

    const fds = getSocketActivationFileDescriptors(
      reactor.base.socketActivationState,
      SOCKET_ACTIVATION_NAME_HTTP,
    )
    logger.debug("using socket activation for http server", { fds })

    for (const fd of fds) {
      server.listen({ fd })
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
