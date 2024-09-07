import { createJsonResponse } from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { HttpsRouter } from "../http-server/values/https-router"
import { NodeListHashSignal } from "../peer-protocol/computed/node-list-hash"

export const RegisterNodeListMetadataHttpHandlerActor = (reactor: Reactor) => {
  const http = reactor.use(HttpsRouter)
  const nodeListHashSignal = reactor.use(NodeListHashSignal)

  return createActor((sig) => {
    http
      .get()
      .path("/nodelist/meta")
      .handler(sig, () => {
        return createJsonResponse({
          hash: nodeListHashSignal.read().toString("base64url"),
        })
      })
  })
}
