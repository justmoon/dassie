import { createActor } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { streamServerService } from "./stream-server"

export const handleSpspRequests = () =>
  createActor(async (sig) => {
    const router = sig.get(routerService)
    const streamServer = await sig.get(streamServerService)

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
