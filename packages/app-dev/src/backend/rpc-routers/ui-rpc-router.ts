import * as trpc from "@trpc/server"
import superjson from "superjson"

import type { Reactor } from "@dassie/lib-reactive"
import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"

import { logsStore } from "../../common/stores/logs"
import { activeTemplateSignal } from "../signals/active-template"
import { peerTrafficTopic } from "../topics/peer-traffic"

export const exposedStores = {
  activeTemplate: activeTemplateSignal,
  logs: logsStore,
  peerTraffic: peerTrafficTopic,
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
      const templateSignal = reactor.useContext(activeTemplateSignal)

      const template = templateSignal.read()

      const nodeCount = new Set(template.flat()).size

      const peers = Array.from({ length: Math.min(nodeCount, 3) })
        .fill(undefined)
        .map(() => Math.floor(Math.random() * nodeCount))

      const uniquePeers = [...new Set(peers)]

      templateSignal.update((nodes) => [...nodes, uniquePeers])
    },
  })

export type UiRpcRouter = typeof uiRpcRouter
