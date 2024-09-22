import { createServer } from "node:http"

import { createNodejsHttpHandlers } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import type { SystemdReactor } from "."
import { HttpRouter } from "../../../../http-server/values/http-router"
import { http as logger } from "../../../../logger/instances"
import { getSocketActivationFileDescriptors } from "./socket-activation"

function handleError(error: unknown) {
  logger.error("http server error", { error })
}

const SOCKET_ACTIVATION_NAME_HTTP = "dassie-http.socket"

export const ServeHttpActor = (reactor: SystemdReactor) => {
  const router = reactor.use(HttpRouter)

  const server = createServer()

  const fds = getSocketActivationFileDescriptors(
    reactor.base.socketActivationState,
    SOCKET_ACTIVATION_NAME_HTTP,
  )
  logger.debug?.("using socket activation for http server", { fds })

  // socket activation can only be done once per process, which is why the call
  // to `server.listen` is outside of the createActor function
  for (const fd of fds) {
    server.listen({ fd })
  }

  return createActor((sig) => {
    const nodejsHandlers = createNodejsHttpHandlers({
      onRequest: async (context) => router.handle(context),
      onError: handleError,
    })

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
