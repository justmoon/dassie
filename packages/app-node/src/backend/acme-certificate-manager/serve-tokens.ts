import { createActor } from "@dassie/lib-reactive"

import { databasePlain } from "../database/open-database"
import { httpRouterService } from "../http-server/serve-http"

export const serveTokens = () =>
  createActor((sig) => {
    const httpRouter = sig.get(httpRouterService)
    const database = sig.use(databasePlain)

    if (!httpRouter) return

    httpRouter.get(
      "/.well-known/acme-challenge/:token",
      (request, response) => {
        const { token } = request.params

        if (!token) {
          response.status(400).send("Bad Request")
          return
        }

        const tokenRow = database.tables.acmeTokens.selectUnique("token", token)

        if (!tokenRow) {
          response.status(404).send("Not Found")
          return
        }

        const keyAuthorization = tokenRow.key_authorization

        response.send(keyAuthorization)
      }
    )
  })
