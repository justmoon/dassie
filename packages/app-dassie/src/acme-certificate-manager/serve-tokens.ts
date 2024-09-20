import {
  BadRequestFailure,
  NotFoundFailure,
  PlainHttpResponse,
} from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { Database } from "../database/open-database"
import { HttpRouter } from "../http-server/values/http-router"

export const ServeTokensActor = (reactor: Reactor) => {
  const httpRouter = reactor.use(HttpRouter)

  return createActor((sig) => {
    const database = sig.reactor.use(Database)

    httpRouter
      .get()
      .path("/.well-known/acme-challenge/:token")
      .handler(sig, ({ parameters: { token } }) => {
        if (!token) {
          return new BadRequestFailure("Bad Request")
        }

        const tokenRow = database.tables.acmeTokens.selectFirst({ token })

        if (!tokenRow) {
          return new NotFoundFailure("Not Found")
        }

        const keyAuthorization = tokenRow.key_authorization

        return new PlainHttpResponse(keyAuthorization)
      })
  })
}
