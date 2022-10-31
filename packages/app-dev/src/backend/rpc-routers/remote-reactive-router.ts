import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"

import { logsStore } from "../../common/stores/logs"
import { activeNodesStore } from "../stores/active-nodes"
import { peerTrafficTopic } from "../topics/peer-traffic"

export const exposedStores = {
  activeNodes: activeNodesStore,
  logs: logsStore,
  peerTraffic: peerTrafficTopic,
} as const

export type ExposedStoresMap = typeof exposedStores
export const remoteReactiveRouter = createRemoteReactiveRouter(exposedStores)

export type RemoteReactiveRouter = typeof remoteReactiveRouter
