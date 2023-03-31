import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { peersComputation } from "../peer-protocol/computed/peers"
import { nodeTableStore } from "../peer-protocol/stores/node-table"

export const registerStatisticsHttpHandler = () =>
  createActor((sig) => {
    const router = sig.get(routerService)

    if (!router) return

    const { reactor } = sig

    router.get("/stats", (_, response) => {
      const nodeTable = reactor.use(nodeTableStore).read()
      const peers = [...reactor.use(peersComputation).read()]
        .map((nodeKey) => {
          const node = nodeTable.get(nodeKey)

          if (!node) return false

          const { nodeId, subnetId, url, nodePublicKey } = node
          return {
            subnetId,
            nodeId,
            url,
            nodePublicKey: Buffer.from(nodePublicKey).toString("base64url"),
          }
        })
        .filter(Boolean)

      respondJson(response, 200, { peers })
    })
  })
