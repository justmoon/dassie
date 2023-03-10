import { createRemoteReactiveRouter } from "@dassie/lib-reactive-trpc/server"

import { logsStore } from "../../common/stores/logs"
import { activeNodesStore } from "../stores/active-nodes"
import { environmentSettingsStore } from "../stores/environment-settings"
import { peeringStateStore } from "../stores/peering-state"
import { peerTrafficTopic } from "../topics/peer-traffic"
import { trpc } from "./trpc"

export const exposedStores = {
  activeNodes: activeNodesStore,
  logs: logsStore,
  peerTraffic: peerTrafficTopic,
  peeringState: peeringStateStore,
  environmentSettings: environmentSettingsStore,
} as const

export type ExposedStoresMap = typeof exposedStores
export const remoteReactiveRouter = createRemoteReactiveRouter(
  trpc,
  exposedStores
)

export type RemoteReactiveRouter = typeof remoteReactiveRouter
