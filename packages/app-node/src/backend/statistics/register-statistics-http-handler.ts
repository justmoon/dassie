import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { httpsRouterService } from "../http-server/serve-https"
import { peersComputation } from "../peer-protocol/computed/peers"
import { nodeTableStore } from "../peer-protocol/stores/node-table"

export const registerStatisticsHttpHandler = () =>
  createActor((sig) => {
    const router = sig.get(httpsRouterService)

    if (!router) return

    const { reactor } = sig

    router.get("/stats", (_, response) => {
      const nodeTable = reactor.use(nodeTableStore).read()
      const peers = [...reactor.use(peersComputation).read()]
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
