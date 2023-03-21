import { respondJson } from "@dassie/lib-http-server"
import { createActor } from "@dassie/lib-reactive"

import { routerService } from "../http-server/serve-http"
import { peerTableStore } from "../peer-protocol/stores/peer-table"

export const registerStatisticsHttpHandler = () =>
  createActor((sig) => {
    const router = sig.get(routerService)

    if (!router) return

    const { reactor } = sig

    router.get("/stats", (_, response) => {
      const peers = [...reactor.use(peerTableStore).read().values()].map(
        ({ nodeId, subnetId, url, nodePublicKey }) => ({
          subnetId,
          nodeId,
          url,
          nodePublicKey: Buffer.from(nodePublicKey).toString("base64url"),
        })
      )

      respondJson(response, 200, { peers })
    })
  })
