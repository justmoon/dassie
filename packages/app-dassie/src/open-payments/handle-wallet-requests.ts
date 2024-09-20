import { cors, createJsonResponse } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { HttpsRouter } from "../http-server/values/https-router"
import { StreamServerServiceActor } from "./stream-server"

export const HandleWalletRequestsActor = () =>
  createActor((sig) => {
    const http = sig.reactor.use(HttpsRouter)
    const streamServer = sig.readAndTrack(StreamServerServiceActor)
    const { hostname } = sig.readKeysAndTrack(DatabaseConfigStore, ["hostname"])

    if (!streamServer) {
      return
    }

    http
      .get()
      .path("/.well-known/pay")
      .use(cors)
      .handler(sig, () => {
        return createJsonResponse({
          id: `https://${hostname}/.well-known/pay`,
          assetCode: "USD",
          assetScale: 9,
          authServer: "https://not-available",
          resourceServer: `https://${hostname}`,
        })
      })
  })
