import * as trpc from "@trpc/server"
import superjson from "superjson"

import type { Reactor } from "@dassie/lib-reactive"
import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"

import { logsStore } from "../../common/stores/logs"
import { activeTemplate } from "../stores/active-template"
import { PeerMessageMetadata, peerTrafficTopic } from "../topics/peer-traffic"

export const exposedStores = {
  activeTemplate,
  logs: logsStore,
} as const

export type ExposedStoresMap = typeof exposedStores
const remoteReactiveRouter = createRemoteReactiveRouter(exposedStores)

export type RemoteReactiveRouter = typeof remoteReactiveRouter

export const uiRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .merge(remoteReactiveRouter)
  .mutation("addRandomNode", {
    resolve({ ctx: reactor }) {
      const templateStore = reactor.useContext(activeTemplate)

      const template = templateStore.read()

      const nodeCount = new Set(template.flat()).size

      const peers = Array.from({ length: Math.min(nodeCount, 3) })
        .fill(undefined)
        .map(() => Math.floor(Math.random() * nodeCount))

      const uniquePeers = [...new Set(peers)]

      templateStore.emit((nodes) => [...nodes, uniquePeers])
    },
  })
  .subscription("peerTraffic", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<PeerMessageMetadata>((sendToClient) => {
        return fromContext(peerTrafficTopic).on((message) => {
          sendToClient.data(message)
        })
      })
    },
  })

export type UiRpcRouter = typeof uiRpcRouter
