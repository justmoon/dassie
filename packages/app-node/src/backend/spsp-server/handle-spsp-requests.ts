import { createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { HttpsRouter } from "../http-server/serve-https"
import { StreamServerServiceActor } from "./stream-server"

export const HandleSpspRequestsActor = () =>
  createActor((sig) => {
    const http = sig.use(HttpsRouter)
    const streamServer = sig.get(StreamServerServiceActor)

    if (!streamServer) {
      return
    }

    http
      .get()
      .path("/.well-known/pay")
      .cors()
      .handler(() => {
        const { destinationAccount, sharedSecret } =
          streamServer.generateAddressAndSecret()
        return createJsonResponse({
          destination_account: destinationAccount,
          shared_secret: sharedSecret.toString("base64"),
        })
      })
  })
