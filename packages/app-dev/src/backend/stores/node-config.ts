import { Reactor, createMapped, createSignal } from "@dassie/lib-reactive"

import { generateNodeConfig } from "../utils/generate-node-config"
import { ActiveNodesStore } from "./active-nodes"
import { EnvironmentSettingsStore } from "./environment-settings"

export const NodeConfigSignals = (reactor: Reactor) =>
  createMapped(reactor, reactor.use(ActiveNodesStore), (nodeId) => {
    const environmentSettings = reactor.use(EnvironmentSettingsStore).read()

    return createSignal(generateNodeConfig(nodeId, environmentSettings))
  })
