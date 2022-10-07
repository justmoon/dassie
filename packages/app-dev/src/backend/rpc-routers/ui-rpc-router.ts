import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"

import { logsStore } from "../../common/stores/logs"
import { activeTemplateSignal } from "../signals/active-template"
import { peerTrafficTopic } from "../topics/peer-traffic"
import { trpc } from "./trpc"

export const exposedStores = {
  activeTemplate: activeTemplateSignal,
  logs: logsStore,
  peerTraffic: peerTrafficTopic,
} as const

export type ExposedStoresMap = typeof exposedStores
const remoteReactiveRouter = createRemoteReactiveRouter(exposedStores)

export type RemoteReactiveRouter = typeof remoteReactiveRouter

export const uiRpcRouter = trpc.mergeRouters(
  trpc.router({
    addRandomNode: trpc.procedure.mutation(({ ctx: { reactor } }) => {
      const templateSignal = reactor.useContext(activeTemplateSignal)

      const template = templateSignal.read()

      const nodeCount = new Set(template.flat()).size

      const peers = Array.from({ length: Math.min(nodeCount, 3) })
        .fill(undefined)
        .map(() => Math.floor(Math.random() * nodeCount))

      const uniquePeers = [...new Set(peers)]

      templateSignal.update((nodes) => [...nodes, uniquePeers])
    }),
  }),
  remoteReactiveRouter
)

export type UiRpcRouter = typeof uiRpcRouter
