import { createActor } from "@dassie/lib-reactive"

import { HttpsRouterServiceActor } from "../http-server/serve-https"
import { StreamServerServiceActor } from "./stream-server"

export const HandleSpspRequestsActor = () =>
  createActor((sig) => {
    const router = sig.get(HttpsRouterServiceActor)
    const streamServer = sig.get(StreamServerServiceActor)

    if (!router || !streamServer) {
      return
    }

    router.get("/.well-known/pay", (_request, response) => {
      response.setHeader("Access-Control-Allow-Origin", "*")
      response.statusCode = 200
      response.setHeader("Content-Type", "application/json")
      const { destinationAccount, sharedSecret } =
        streamServer.generateAddressAndSecret()
      response.end(
        JSON.stringify({
          destination_account: destinationAccount,
          shared_secret: sharedSecret.toString("base64"),
        }),
      )
    })
  })
