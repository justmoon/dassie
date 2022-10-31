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
export const remoteReactiveRouter = createRemoteReactiveRouter(exposedStores)

export type RemoteReactiveRouter = typeof remoteReactiveRouter
