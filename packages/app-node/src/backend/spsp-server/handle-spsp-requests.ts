import { createActor } from "@dassie/lib-reactive"

import { httpsRouterService } from "../http-server/serve-https"
import { streamServerService } from "./stream-server"

export const handleSpspRequests = () =>
  createActor((sig) => {
    const router = sig.get(httpsRouterService)
    const streamServer = sig.get(streamServerService)

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
        })
      )
    })
  })
