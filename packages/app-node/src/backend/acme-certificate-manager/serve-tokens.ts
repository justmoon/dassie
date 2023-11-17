import { createActor } from "@dassie/lib-reactive"

import { Database } from "../database/open-database"
import { HttpRouterServiceActor } from "../http-server/serve-http"

export const ServeTokensActor = () =>
  createActor((sig) => {
    const httpRouter = sig.readAndTrack(HttpRouterServiceActor)
    const database = sig.reactor.use(Database)

    if (!httpRouter) return

    httpRouter.get(
      "/.well-known/acme-challenge/:token",
      (request, response) => {
        const { token } = request.params

        if (!token) {
          response.status(400).send("Bad Request")
          return
        }

        const tokenRow = database.tables.acmeTokens.selectFirst({ token })

        if (!tokenRow) {
          response.status(404).send("Not Found")
          return
        }

        const keyAuthorization = tokenRow.key_authorization

        response.send(keyAuthorization)
      },
    )
  })
