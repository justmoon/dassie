import * as trpc from "@trpc/server"
import superjson from "superjson"

import type { Reactor } from "@dassie/lib-reactive"
import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"

import { config } from "../config"
import { IndexedLogLine, indexedLogLineTopic } from "../features/logs"
import { activeTemplate } from "../stores/active-template"
import { logsStore } from "../stores/logs"
import { PeerMessageMetadata, peerTrafficTopic } from "../topics/peer-traffic"

export const exposedStores = {
  activeTemplate,
} as const

export type ExposedStoresMap = typeof exposedStores
const remoteReactiveRouter = createRemoteReactiveRouter(exposedStores)

export type RemoteReactiveRouter = typeof remoteReactiveRouter

export const uiRpcRouter = trpc
  .router<Reactor>()
  .transformer(superjson)
  .merge(remoteReactiveRouter)
  .query("config", {
    resolve() {
      return config
    },
  })
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
  .subscription("logs", {
    resolve({ ctx: { useContext: fromContext } }) {
      return new trpc.Subscription<IndexedLogLine>((sendToClient) => {
        const previousLogs = fromContext(logsStore).read()

        for (const logLine of previousLogs) {
          sendToClient.data(logLine)
        }

        return fromContext(indexedLogLineTopic).on((logLine) => {
          sendToClient.data(logLine)
        })
      })
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
