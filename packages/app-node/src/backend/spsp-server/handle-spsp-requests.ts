import { cors, createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { HttpsRouter } from "../http-server/serve-https"
import { StreamServerServiceActor } from "./stream-server"

export const HandleSpspRequestsActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const streamServer = sig.readAndTrack(StreamServerServiceActor)

    if (!streamServer) {
      return
    }

    http
      .get()
      .path("/.well-known/pay")
      .use(cors)
      .handler(sig, () => {
        const { destinationAccount, sharedSecret } =
          streamServer.generateAddressAndSecret()
        return createJsonResponse({
          destination_account: destinationAccount,
          shared_secret: sharedSecret.toString("base64"),
        })
      })
  })
