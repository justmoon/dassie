import { respondJson } from "@dassie/lib-http-server"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { HttpsRouterServiceActor } from "../http-server/serve-https"
import { PeersSignal } from "../peer-protocol/computed/peers"
import { NodeTableStore } from "../peer-protocol/stores/node-table"

export const RegisterStatisticsHttpHandlerActor = (reactor: Reactor) =>
  createActor((sig) => {
    const router = sig.get(HttpsRouterServiceActor)

    if (!router) return

    router.get("/stats", (_, response) => {
      const nodeTable = reactor.use(NodeTableStore).read()
      const peers = [...reactor.use(PeersSignal).read()]
        .map((nodeKey) => {
          const node = nodeTable.get(nodeKey)

          if (!node) return false

          const { nodeId, url, nodePublicKey } = node
          return {
            nodeId,
            url,
            nodePublicKey: Buffer.from(nodePublicKey).toString("base64url"),
          }
        })
        .filter(Boolean)

      respondJson(response, 200, {
        version: __DASSIE_VERSION__,
        peers,
      })
    })
  })
