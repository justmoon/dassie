import { createJsonResponse } from "@dassie/lib-http-server"
import { type Reactor, createActor } from "@dassie/lib-reactive"

import { DatabaseConfigStore } from "../config/database-config"
import { NodePublicKeySignal } from "../crypto/computed/node-public-key"
import { HttpsRouter } from "../http-server/values/https-router"
import { NodeIdSignal } from "../ilp-connector/computed/node-id"
import { PeersSignal } from "../peer-protocol/computed/peers"
import { NodeTableStore } from "../peer-protocol/stores/node-table"

export const RegisterStatisticsHttpHandlerActor = (reactor: Reactor) => {
  const nodeIdSignal = reactor.use(NodeIdSignal)
  const nodeTableStore = reactor.use(NodeTableStore)
  const peersSignal = reactor.use(PeersSignal)
  const nodePublicKeySignal = reactor.use(NodePublicKeySignal)
  const databaseConfigStore = reactor.use(DatabaseConfigStore)
  const http = reactor.use(HttpsRouter)

  return createActor((sig) => {
    http
      .get()
      .path("/stats")
      .handler(sig, () => {
        const nodeTable = nodeTableStore.read()
        const peers = [...peersSignal.read()]
          .map((nodeKey) => {
            const node = nodeTable.get(nodeKey)

            if (!node) return false

            const { nodeId, linkState } = node

            return {
              nodeId,
              url: linkState?.url ?? null,
              publicKey:
                linkState ?
                  Buffer.from(linkState.publicKey).toString("base64url")
                : null,
            }
          })
          .filter(Boolean)

        return createJsonResponse({
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
