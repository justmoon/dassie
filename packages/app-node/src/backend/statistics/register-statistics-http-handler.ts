import { respondJson } from "@dassie/lib-http-server"
import { Reactor, createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { NodePublicKeySignal } from "../crypto/computed/node-public-key"
import { HttpsRouterServiceActor } from "../http-server/serve-https"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { PeersSignal } from "../peer-protocol/computed/peers"
import { NodeTableStore } from "../peer-protocol/stores/node-table"

export const RegisterStatisticsHttpHandlerActor = (reactor: Reactor) => {
  const nodeIdSignal = reactor.use(NodeIdSignal)
  const nodeTableStore = reactor.use(NodeTableStore)
  const peersSignal = reactor.use(PeersSignal)
  const nodePublicKeySignal = reactor.use(NodePublicKeySignal)
  const databaseConfigStore = reactor.use(DatabaseConfigStore)

  return createActor((sig) => {
    const router = sig.get(HttpsRouterServiceActor)

    if (!router) return

    router.get("/stats", (_, response) => {
      const nodeTable = nodeTableStore.read()
      const peers = [...peersSignal.read()]
        .map((nodeKey) => {
          const node = nodeTable.get(nodeKey)

          if (!node) return false

          const { nodeId, linkState } = node

          return {
            nodeId,
            url: linkState?.url,
            publicKey: linkState
              ? Buffer.from(linkState.publicKey).toString("base64url")
              : undefined,
          }
        })
        .filter(Boolean)

      respondJson(response, 200, {
        version: __DASSIE_VERSION__,
        node: {
          id: nodeIdSignal.read(),
          url: databaseConfigStore.read().url,
          publicKey: Buffer.from(nodePublicKeySignal.read()).toString(
            "base64url",
          ),
        },
        peers,
      })
    })
  })
}
