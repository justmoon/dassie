import { NotFoundFailure, RedirectResponse } from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { HasTlsSignal } from "../config/computed/has-tls"
import { DatabaseConfigStore } from "../config/database-config"
import { HttpRouter } from "./values/http-router"

export const RedirectToHttpsActor = (reactor: Reactor) => {
  const router = reactor.use(HttpRouter)

  return createActor((sig) => {
    const { httpsPort } = sig.readKeysAndTrack(DatabaseConfigStore, [
      "httpsPort",
    ])
    const hasTls = sig.readAndTrack(HasTlsSignal)

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
  })
}
